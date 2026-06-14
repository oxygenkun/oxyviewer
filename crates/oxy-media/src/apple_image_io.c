#include <CoreFoundation/CoreFoundation.h>
#include <CoreGraphics/CoreGraphics.h>
#include <ImageIO/ImageIO.h>
#include <stdint.h>
#include <stdlib.h>
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

void oxy_apple_image_io_free(void *pixels) { free(pixels); }
