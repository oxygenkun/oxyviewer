#include <CoreFoundation/CoreFoundation.h>
#include <CoreGraphics/CoreGraphics.h>
#include <ImageIO/ImageIO.h>
#include <Accelerate/Accelerate.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>

static CGImageSourceRef oxy_image_source(const uint8_t *path, size_t path_len) {
  CFURLRef url = CFURLCreateFromFileSystemRepresentation(
      kCFAllocatorDefault, path, path_len, false);
  if (url == NULL) {
    return NULL;
  }
  CGImageSourceRef source =
      CGImageSourceCreateWithURL(url, NULL);
  CFRelease(url);
  return source;
}

int32_t oxy_apple_image_io_can_decode(const uint8_t *path, size_t path_len) {
  CGImageSourceRef source = oxy_image_source(path, path_len);
  if (source == NULL) {
    return 0;
  }
  int32_t result =
      CGImageSourceGetCount(source) > 0 && CGImageSourceGetType(source) != NULL;
  CFRelease(source);
  return result;
}

int32_t oxy_apple_image_io_decode_rgba8(const uint8_t *path, size_t path_len,
                                        uint32_t max_size, uint8_t **pixels,
                                        size_t *pixels_len, uint32_t *width,
                                        uint32_t *height) {
  CGImageSourceRef source = oxy_image_source(path, path_len);
  if (source == NULL) {
    return 1;
  }

  CFNumberRef size = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type,
                                    &max_size);
  const void *keys[] = {
      kCGImageSourceCreateThumbnailFromImageAlways,
      kCGImageSourceCreateThumbnailWithTransform,
      kCGImageSourceThumbnailMaxPixelSize,
      kCGImageSourceShouldCacheImmediately,
  };
  const void *values[] = {kCFBooleanTrue, kCFBooleanTrue, size, kCFBooleanTrue};
  CFDictionaryRef options = CFDictionaryCreate(
      kCFAllocatorDefault, keys, values, 4, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  CGImageRef image = CGImageSourceCreateThumbnailAtIndex(source, 0, options);
  CFRelease(options);
  CFRelease(size);
  CFRelease(source);
  if (image == NULL) {
    return 2;
  }

  size_t image_width = CGImageGetWidth(image);
  size_t image_height = CGImageGetHeight(image);
  if (image_width > UINT32_MAX || image_height > UINT32_MAX ||
      image_width > SIZE_MAX / 4 ||
      image_height > SIZE_MAX / (image_width * 4)) {
    CGImageRelease(image);
    return 3;
  }
  size_t row_bytes = image_width * 4;
  size_t length = row_bytes * image_height;
  uint8_t *data = malloc(length);
  if (data == NULL) {
    CGImageRelease(image);
    return 4;
  }

  CGColorSpaceRef color_space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGBitmapInfo bitmap_info =
      kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big;
  CGContextRef context =
      CGBitmapContextCreate(data, image_width, image_height, 8, row_bytes,
                            color_space, bitmap_info);
  CGColorSpaceRelease(color_space);
  if (context == NULL) {
    free(data);
    CGImageRelease(image);
    return 5;
  }

  CGContextDrawImage(context, CGRectMake(0, 0, image_width, image_height),
                     image);
  CGContextRelease(context);
  CGImageRelease(image);
  *pixels = data;
  *pixels_len = length;
  *width = (uint32_t)image_width;
  *height = (uint32_t)image_height;
  return 0;
}

static CGImageRef oxy_image_in_srgb(CGImageRef image) {
  size_t width = CGImageGetWidth(image);
  size_t height = CGImageGetHeight(image);
  if (width == 0 || height == 0 || width > SIZE_MAX / 4 ||
      height > SIZE_MAX / (width * 4)) {
    return NULL;
  }
  CGColorSpaceRef color_space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGBitmapInfo bitmap_info =
      kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big;
  CGContextRef context = CGBitmapContextCreate(
      NULL, width, height, 8, width * 4, color_space, bitmap_info);
  CGColorSpaceRelease(color_space);
  if (context == NULL) {
    return NULL;
  }
  CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
  CGImageRef converted = CGBitmapContextCreateImage(context);
  CGContextRelease(context);
  return converted;
}

int32_t oxy_apple_image_io_render_jpeg(
    const uint8_t *source_path, size_t source_path_len,
    const uint8_t *destination_path, size_t destination_path_len,
    uint32_t max_size, uint8_t quality) {
  CGImageSourceRef source = oxy_image_source(source_path, source_path_len);
  if (source == NULL || CGImageSourceGetCount(source) == 0) {
    if (source != NULL) {
      CFRelease(source);
    }
    return 1;
  }

  if (max_size == 0) {
    CFDictionaryRef properties =
        CGImageSourceCopyPropertiesAtIndex(source, 0, NULL);
    if (properties != NULL) {
      CFNumberRef width = CFDictionaryGetValue(properties,
                                               kCGImagePropertyPixelWidth);
      CFNumberRef height = CFDictionaryGetValue(properties,
                                                kCGImagePropertyPixelHeight);
      int64_t width_value = 0;
      int64_t height_value = 0;
      if (width != NULL) {
        CFNumberGetValue(width, kCFNumberSInt64Type, &width_value);
      }
      if (height != NULL) {
        CFNumberGetValue(height, kCFNumberSInt64Type, &height_value);
      }
      int64_t longest = width_value > height_value ? width_value : height_value;
      if (longest > 0 && longest <= UINT32_MAX) {
        max_size = (uint32_t)longest;
      }
      CFRelease(properties);
    }
  }
  if (max_size == 0) {
    CFRelease(source);
    return 2;
  }

  CFNumberRef size = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type,
                                    &max_size);
  const void *keys[] = {
      kCGImageSourceCreateThumbnailFromImageAlways,
      kCGImageSourceCreateThumbnailWithTransform,
      kCGImageSourceThumbnailMaxPixelSize,
      kCGImageSourceShouldCacheImmediately,
  };
  const void *values[] = {kCFBooleanTrue, kCFBooleanTrue, size, kCFBooleanTrue};
  CFDictionaryRef decode_options = CFDictionaryCreate(
      kCFAllocatorDefault, keys, values, 4, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  CGImageRef image =
      CGImageSourceCreateThumbnailAtIndex(source, 0, decode_options);
  CFRelease(decode_options);
  CFRelease(size);
  CFRelease(source);
  if (image == NULL) {
    return 3;
  }
  CGImageRef srgb_image = oxy_image_in_srgb(image);
  CGImageRelease(image);
  if (srgb_image == NULL) {
    return 4;
  }

  CFURLRef destination_url = CFURLCreateFromFileSystemRepresentation(
      kCFAllocatorDefault, destination_path, destination_path_len, false);
  if (destination_url == NULL) {
    CGImageRelease(srgb_image);
    return 5;
  }
  CGImageDestinationRef destination = CGImageDestinationCreateWithURL(
      destination_url, CFSTR("public.jpeg"), 1, NULL);
  CFRelease(destination_url);
  if (destination == NULL) {
    CGImageRelease(srgb_image);
    return 6;
  }
  float quality_value = (float)quality / 100.0f;
  CFNumberRef quality_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberFloatType, &quality_value);
  const void *output_keys[] = {kCGImageDestinationLossyCompressionQuality};
  const void *output_values[] = {quality_number};
  CFDictionaryRef output_options = CFDictionaryCreate(
      kCFAllocatorDefault, output_keys, output_values, 1,
      &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
  CGImageDestinationAddImage(destination, srgb_image, output_options);
  bool finalized = CGImageDestinationFinalize(destination);
  CFRelease(output_options);
  CFRelease(quality_number);
  CFRelease(destination);
  CGImageRelease(srgb_image);
  return finalized ? 0 : 7;
}

int32_t oxy_apple_image_io_write_jpeg(const uint8_t *path, size_t path_len,
                                      const uint8_t *pixels, size_t pixels_len,
                                      uint32_t width, uint32_t height,
                                      uint8_t quality) {
  if (width == 0 || height == 0 || width > SIZE_MAX / 4 ||
      height > SIZE_MAX / ((size_t)width * 4) ||
      pixels_len != (size_t)width * (size_t)height * 4) {
    return 1;
  }

  CFURLRef url = CFURLCreateFromFileSystemRepresentation(
      kCFAllocatorDefault, path, path_len, false);
  if (url == NULL) {
    return 2;
  }
  CGImageDestinationRef destination =
      CGImageDestinationCreateWithURL(url, CFSTR("public.jpeg"), 1, NULL);
  CFRelease(url);
  if (destination == NULL) {
    return 3;
  }

  CGDataProviderRef provider =
      CGDataProviderCreateWithData(NULL, pixels, pixels_len, NULL);
  CGColorSpaceRef color_space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGBitmapInfo bitmap_info =
      kCGImageAlphaLast | kCGBitmapByteOrder32Big;
  CGImageRef image = CGImageCreate(
      width, height, 8, 32, (size_t)width * 4, color_space, bitmap_info,
      provider, NULL, false, kCGRenderingIntentDefault);
  CGColorSpaceRelease(color_space);
  CGDataProviderRelease(provider);
  if (image == NULL) {
    CFRelease(destination);
    return 4;
  }

  float quality_value = (float)quality / 100.0f;
  CFNumberRef quality_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberFloatType, &quality_value);
  const void *keys[] = {kCGImageDestinationLossyCompressionQuality};
  const void *values[] = {quality_number};
  CFDictionaryRef properties = CFDictionaryCreate(
      kCFAllocatorDefault, keys, values, 1, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  CGImageDestinationAddImage(destination, image, properties);
  bool finalized = CGImageDestinationFinalize(destination);
  CFRelease(properties);
  CFRelease(quality_number);
  CGImageRelease(image);
  CFRelease(destination);
  return finalized ? 0 : 5;
}

int32_t oxy_apple_image_io_transcode_jpeg(
    const uint8_t *source_path, size_t source_path_len,
    const uint8_t *destination_path, size_t destination_path_len,
    uint8_t quality) {
  CGImageSourceRef source = oxy_image_source(source_path, source_path_len);
  if (source == NULL || CGImageSourceGetCount(source) == 0) {
    if (source != NULL) {
      CFRelease(source);
    }
    return 1;
  }

  CFURLRef destination_url = CFURLCreateFromFileSystemRepresentation(
      kCFAllocatorDefault, destination_path, destination_path_len, false);
  if (destination_url == NULL) {
    CFRelease(source);
    return 2;
  }
  CGImageDestinationRef destination = CGImageDestinationCreateWithURL(
      destination_url, CFSTR("public.jpeg"), 1, NULL);
  CFRelease(destination_url);
  if (destination == NULL) {
    CFRelease(source);
    return 3;
  }

  float quality_value = (float)quality / 100.0f;
  CFNumberRef quality_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberFloatType, &quality_value);

  CGImageRef image = CGImageSourceCreateImageAtIndex(source, 0, NULL);
  if (image == NULL) {
    CFRelease(quality_number);
    CFRelease(destination);
    CFRelease(source);
    return 4;
  }
  CFMutableDictionaryRef output_properties = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 2, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  CFDictionarySetValue(output_properties,
                       kCGImageDestinationLossyCompressionQuality,
                       quality_number);
  CFDictionaryRef source_properties =
      CGImageSourceCopyPropertiesAtIndex(source, 0, NULL);
  if (source_properties != NULL) {
    CFTypeRef orientation =
        CFDictionaryGetValue(source_properties, kCGImagePropertyOrientation);
    if (orientation != NULL) {
      CFDictionarySetValue(output_properties, kCGImagePropertyOrientation,
                           orientation);
    }
    CFRelease(source_properties);
  }

  // Keep the source decode and JPEG encode inside ImageIO. No 125 MiB RGBA
  // allocation crosses the native boundary back into Rust. Copying the source
  // properties retains the HEIF display orientation and color metadata.
  CGImageDestinationAddImage(destination, image, output_properties);
  bool finalized = CGImageDestinationFinalize(destination);
  CFRelease(output_properties);
  CGImageRelease(image);
  CFRelease(quality_number);
  CFRelease(destination);
  CFRelease(source);
  return finalized ? 0 : 5;
}

int32_t oxy_apple_image_io_sharpen_rgba8(uint8_t *pixels, size_t pixels_len,
                                         uint32_t width, uint32_t height) {
  if (pixels == NULL || width == 0 || height == 0 || width > SIZE_MAX / 4 ||
      height > SIZE_MAX / ((size_t)width * 4) ||
      pixels_len != (size_t)width * (size_t)height * 4) {
    return 1;
  }

  uint8_t *output = malloc(pixels_len);
  if (output == NULL) {
    return 2;
  }
  vImage_Buffer source = {
      .data = pixels,
      .height = height,
      .width = width,
      .rowBytes = (size_t)width * 4,
  };
  vImage_Buffer destination = {
      .data = output,
      .height = height,
      .width = width,
      .rowBytes = (size_t)width * 4,
  };
  // output = source + 0.2 * (4 * source - left - right - up - down)
  const int16_t kernel[9] = {0, -1, 0, -1, 9, -1, 0, -1, 0};
  vImage_Error error = vImageConvolve_ARGB8888(
      &source, &destination, NULL, 0, 0, kernel, 3, 3, 5, NULL,
      kvImageEdgeExtend);
  if (error == kvImageNoError) {
    memcpy(pixels, output, pixels_len);
  }
  free(output);
  return error == kvImageNoError ? 0 : 3;
}

void oxy_apple_image_io_free(void *pixels) { free(pixels); }
