interface BrowserImageSize {
  width: number;
  height: number;
}

interface BrowserImageResource extends BrowserImageSize {
  decodedBytes: number;
  /**
   * Keep the decoded browser resource alive after its virtualized DOM node is
   * unmounted. Without this reference, WebView is free to discard the image
   * and the next mount can hit the Tauri asset protocol again.
   */
  image?: HTMLImageElement;
}

export const BROWSER_IMAGE_RESOURCE_LIMITS = Object.freeze({
  maxEntries: 1_024,
  maxDecodedBytes: 512 * 1024 * 1024,
});

// The owner of this cache is the WebView session, not an individual virtual
// row. The Map's insertion order tracks least-recently-used resources.
const readyImages = new Map<string, BrowserImageResource>();
let activeResourceScope: string | undefined;
let retainedDecodedBytes = 0;
let protectedUrls = new Set<string>();

/** Prefer selected and nearby resources, while always respecting the hard budget. */
export function protectBrowserImages(urls: readonly string[]): void {
  protectedUrls = new Set(urls);
}

export function touchBrowserImage(url: string): void {
  const resource = readyImages.get(url);
  if (!resource) return;
  readyImages.delete(url);
  readyImages.set(url, resource);
}

function estimateDecodedBytes(size: BrowserImageSize): number {
  const width = Number.isFinite(size.width) ? Math.max(0, Math.floor(size.width)) : 0;
  const height = Number.isFinite(size.height) ? Math.max(0, Math.floor(size.height)) : 0;
  return width * height * 4;
}

function evictOldestResourcesUntilWithinBudget(): void {
  while (
    readyImages.size > BROWSER_IMAGE_RESOURCE_LIMITS.maxEntries
    || retainedDecodedBytes > BROWSER_IMAGE_RESOURCE_LIMITS.maxDecodedBytes
  ) {
    const oldestUrl = [...readyImages.keys()].find((url) => !protectedUrls.has(url))
      ?? readyImages.keys().next().value;
    if (!oldestUrl) return;
    const oldest = readyImages.get(oldestUrl);
    readyImages.delete(oldestUrl);
    retainedDecodedBytes -= oldest?.decodedBytes ?? 0;
  }
}

/** Highest-quality decoded candidate wins without starting another decode. */
export function firstReadyBrowserImage(urls: readonly (string | undefined)[]): string | undefined {
  return urls.find((url): url is string => Boolean(url && readyImages.has(url)));
}

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
  const decodedBytes = estimateDecodedBytes(size);
  retainedDecodedBytes += decodedBytes - (current?.decodedBytes ?? 0);
  readyImages.delete(url);
  readyImages.set(url, {
    ...size,
    decodedBytes,
    image: image ?? current?.image,
  });
  evictOldestResourcesUntilWithinBudget();
}

/** Discards all retained resources in response to an explicit app update. */
export function clearBrowserImageResources(): void {
  readyImages.clear();
  protectedUrls.clear();
  retainedDecodedBytes = 0;
}

/** Keeps resources across virtual mounts, but not across asset browsers. */
export function setBrowserImageResourceScope(scope: string): void {
  if (activeResourceScope !== undefined && activeResourceScope !== scope) {
    clearBrowserImageResources();
  }
  activeResourceScope = scope;
}

export function discardBrowserImageResource(url: string | undefined): void {
  if (!url) return;
  const resource = readyImages.get(url);
  if (!resource) return;
  readyImages.delete(url);
  retainedDecodedBytes -= resource.decodedBytes;
}

/** Loads and decodes an image so a later loupe switch can paint it immediately. */
export function preloadBrowserImage(url: string, signal?: AbortSignal): Promise<void> {
  if (readyImages.has(url)) {
    touchBrowserImage(url);
    return Promise.resolve();
  }

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
          if (signal?.aborted) return;
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
