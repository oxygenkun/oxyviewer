import type { AssetKind, AssetSummary, RenderLevel } from "../types";

export type RenderPlatform = "windows" | "macos" | "linux";
export type RenderSurface = "thumbnail" | "loupe";

export type RenderMethod =
  | { type: "originalImage" }
  | { type: "generatedImage"; requestLevel: RenderLevel }
  | { type: "heifTiles" };

type ConfiguredRenderMethod = RenderMethod | { type: "reuse"; level: RenderLevel };
type RenderProfile = Record<RenderLevel, ConfiguredRenderMethod>;

export interface RenderStep {
  /** Interaction semantics. Never infer this from the artifact's dimensions. */
  level: RenderLevel;
  /** Concrete renderer selected by the file-type/platform profile. */
  method: RenderMethod;
}

/**
 * The interaction graph is deliberately format- and pixel-independent.
 * A list/grid asks for thumbnail; loupe first establishes a persistent preview
 * layer and then advances to full. Profiles below decide how each node is
 * rendered and may map several nodes to the same artifact.
 */
const SURFACE_LEVELS: Record<RenderSurface, readonly RenderLevel[]> = {
  thumbnail: ["thumbnail"],
  loupe: ["preview", "full"],
};

const originalProfile: RenderProfile = {
  thumbnail: { type: "originalImage" },
  preview: { type: "originalImage" },
  full: { type: "originalImage" },
};

const rawProfile: RenderProfile = {
  thumbnail: { type: "generatedImage", requestLevel: "thumbnail" },
  preview: { type: "generatedImage", requestLevel: "preview" },
  full: { type: "generatedImage", requestLevel: "full" },
};

const tiffProfile: RenderProfile = {
  thumbnail: { type: "generatedImage", requestLevel: "thumbnail" },
  preview: { type: "generatedImage", requestLevel: "preview" },
  full: { type: "generatedImage", requestLevel: "full" },
};

const heifTileProfile: RenderProfile = {
  thumbnail: { type: "generatedImage", requestLevel: "thumbnail" },
  // Sony HIF's 160x120 camera JPEG fulfills both semantic levels. Resolving
  // this alias gives grid and loupe the exact same React Query cache identity.
  preview: { type: "reuse", level: "thumbnail" },
  full: { type: "heifTiles" },
};

const heifJpegProfile: RenderProfile = {
  ...heifTileProfile,
  // ImageIO can transcode the source HEIF directly to a full-resolution JPEG
  // without transferring a full RGBA buffer through Rust or the WebView.
  full: { type: "generatedImage", requestLevel: "full" },
};

// Keep platform as an explicit policy dimension even where the qualified
// strategy is currently identical. A platform may diverge only after its
// native path has its own fixture-backed performance and fidelity evidence.
const heifProfiles: Record<RenderPlatform, RenderProfile> = {
  windows: heifTileProfile,
  macos: heifJpegProfile,
  linux: heifTileProfile,
};

export function runtimeRenderPlatform(
  userAgent = globalThis.navigator?.userAgent ?? "",
): RenderPlatform {
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  if (/Linux/i.test(userAgent)) return "linux";
  throw new Error(`unsupported render platform: ${userAgent || "unknown user agent"}`);
}

function renderProfile(kind: AssetKind, platform: RenderPlatform): RenderProfile {
  if (kind === "raw") return rawProfile;
  if (kind === "heif") return heifProfiles[platform];
  if (kind === "tiff") return tiffProfile;
  return originalProfile;
}

function resolveMethod(profile: RenderProfile, level: RenderLevel): RenderMethod {
  const method = profile[level];
  if (method.type !== "reuse") return method;
  const reused = profile[method.level];
  if (reused.type === "reuse") {
    throw new Error(`render profile contains a reuse cycle at ${method.level}`);
  }
  return reused;
}

export function renderPlan(
  kind: AssetKind,
  surface: RenderSurface,
  platform = runtimeRenderPlatform(),
): RenderStep[] {
  const profile = renderProfile(kind, platform);
  return SURFACE_LEVELS[surface].map((level) => ({
    level,
    method: resolveMethod(profile, level),
  }));
}

/**
 * Returns a cheaper loupe safety net when its normal preview uses a distinct
 * artifact. Filmstrips already request this step, so the loupe can reuse it
 * while a larger RAW/TIFF preview is unavailable or fails to decode.
 */
export function loupeThumbnailFallback(
  kind: AssetKind,
  platform = runtimeRenderPlatform(),
): RenderStep | undefined {
  const thumbnail = renderPlan(kind, "thumbnail", platform)[0];
  const preview = renderPlan(kind, "loupe", platform)[0];
  return renderMethodKey(thumbnail.method) === renderMethodKey(preview.method)
    ? undefined
    : thumbnail;
}

/** Stable artifact identity for query reuse; scheduling priority is not data identity. */
export function renderMethodKey(method: RenderMethod): string {
  return method.type === "generatedImage"
    ? `${method.type}:${method.requestLevel}`
    : method.type;
}

export function assetRenderQueryKey(asset: AssetSummary, method: RenderMethod) {
  return ["asset-render", asset.id, asset.modifiedAtMs, renderMethodKey(method)] as const;
}
