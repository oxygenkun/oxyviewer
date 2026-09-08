/** Normalizes the controlled media scheme for the current WebView engine. */
export function mediaProtocolUrl(url: string): string {
  return navigator.userAgent.includes("Windows")
    ? url.replace("oxy-media://localhost", "http://oxy-media.localhost")
    : url;
}
