interface BrowserImageSize {
  width: number;
  height: number;
}

interface BrowserImageResource extends BrowserImageSize {
  /**
   * Keep the decoded browser resource alive after its virtualized DOM node is
   * unmounted. Without this reference, WebView is free to discard the image
   * and the next mount can hit the Tauri asset protocol again.
   */
  image?: HTMLImageElement;
}

// The owner of this cache is the WebView session, not an individual virtual
// row. Entries are only discarded when the application explicitly invalidates
// image resources (or when the WebView itself goes away).
const readyImages = new Map<string, BrowserImageResource>();
let activeResourceScope: string | undefined;

export function isBrowserImageReady(url: string): boolean {
  return readyImages.has(url);
}

export function browserImageSourceWhenEnabled(
  url: string | undefined,
  enabled: boolean,
): string | undefined {
  return url && (enabled || readyImages.has(url)) ? url : undefined;
}

export function getBrowserImageSize(url: string): BrowserImageSize | undefined {
  return readyImages.get(url);
}

export function markBrowserImageReady(
  url: string,
  size: BrowserImageSize,
  image?: HTMLImageElement,
): void {
  const current = readyImages.get(url);
  readyImages.set(url, {
    ...size,
    image: image ?? current?.image,
  });
}

/** Discards retained resources only in response to an explicit app update. */
export function clearBrowserImageResources(): void {
  readyImages.clear();
}

/** Keeps resources across virtual mounts, but not across asset browsers. */
export function setBrowserImageResourceScope(scope: string): void {
  if (activeResourceScope !== undefined && activeResourceScope !== scope) {
    clearBrowserImageResources();
  }
  activeResourceScope = scope;
}

export function discardBrowserImageResource(url: string | undefined): void {
  if (url) readyImages.delete(url);
}

/** Loads and decodes an image so a later loupe switch can paint it immediately. */
export function preloadBrowserImage(url: string, signal?: AbortSignal): Promise<void> {
  if (readyImages.has(url)) return Promise.resolve();

  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason ?? new DOMException("Aborted", "AbortError"));
      return;
    }
    const image = new Image();
    const cleanup = () => {
      image.onload = null;
      image.onerror = null;
      signal?.removeEventListener("abort", abort);
    };
    const abort = () => {
      image.src = "";
      cleanup();
      reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
    };
    image.onload = () => {
      void image.decode()
        .catch(() => undefined)
        .then(() => {
          markBrowserImageReady(url, {
            width: image.naturalWidth,
            height: image.naturalHeight,
          }, image);
          cleanup();
          resolve();
        });
    };
    image.onerror = () => {
      cleanup();
      reject(new Error(`failed to preload ${url}`));
    };
    signal?.addEventListener("abort", abort, { once: true });
    image.src = url;
  });
}
