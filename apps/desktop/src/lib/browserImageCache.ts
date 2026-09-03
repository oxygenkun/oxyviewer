interface BrowserImageSize {
  width: number;
  height: number;
}

const readyImages = new Map<string, BrowserImageSize>();
const MAX_READY_IMAGES = 512;

export function isBrowserImageReady(url: string): boolean {
  return readyImages.has(url);
}

export function getBrowserImageSize(url: string): BrowserImageSize | undefined {
  return readyImages.get(url);
}

export function markBrowserImageReady(url: string, size: BrowserImageSize): void {
  readyImages.delete(url);
  readyImages.set(url, size);
  if (readyImages.size <= MAX_READY_IMAGES) return;
  const oldest = readyImages.keys().next().value;
  if (oldest) readyImages.delete(oldest);
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
          });
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
