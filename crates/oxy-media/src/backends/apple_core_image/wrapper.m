#import <CoreFoundation/CoreFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreImage/CoreImage.h>
#import <Foundation/Foundation.h>
#import <stdint.h>

static NSURL *oxy_file_url(const uint8_t *path, size_t path_len) {
  CFURLRef url = CFURLCreateFromFileSystemRepresentation(
      kCFAllocatorDefault, path, path_len, false);
  return CFBridgingRelease(url);
}

int32_t oxy_apple_core_image_render_raw_jpeg(
    const uint8_t *source_path, size_t source_path_len,
    const uint8_t *destination_path, size_t destination_path_len,
    uint32_t max_size, uint8_t quality) {
  @autoreleasepool {
    @try {
    NSURL *source_url = oxy_file_url(source_path, source_path_len);
    NSURL *destination_url = oxy_file_url(destination_path, destination_path_len);
    if (source_url == nil || destination_url == nil) {
      return 1;
    }

    CIRAWFilter *filter = nil;
    if (@available(macOS 12.0, *)) {
      filter = [CIRAWFilter filterWithImageURL:source_url];
    }
    if (filter == nil) {
      return 2;
    }
    CGSize native_size = filter.nativeSize;
    if (native_size.width <= 0 || native_size.height <= 0) {
      return 3;
    }
    CIImage *image = filter.outputImage;
    if (image == nil || CGRectIsEmpty(image.extent) || CGRectIsInfinite(image.extent)) {
      return 4;
    }
    CGRect extent = CGRectIntegral(image.extent);
    if (max_size > 0) {
      CGFloat longest = MAX(extent.size.width, extent.size.height);
      CGFloat scale = MIN(1.0, (CGFloat)max_size / longest);
      image = [image imageByApplyingTransform:CGAffineTransformMakeScale(scale, scale)];
      extent = CGRectIntegral(image.extent);
    }
    if (extent.origin.x != 0 || extent.origin.y != 0) {
      image = [image imageByApplyingTransform:CGAffineTransformMakeTranslation(
          -extent.origin.x, -extent.origin.y)];
    }

    static CIContext *context;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      context = [CIContext contextWithOptions:@{
        kCIContextCacheIntermediates : @NO,
      }];
    });
    CGColorSpaceRef color_space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    NSDictionary *options = @{
      (__bridge NSString *)kCGImageDestinationLossyCompressionQuality :
          @((double)quality / 100.0),
    };
    NSError *error = nil;
    BOOL written = [context writeJPEGRepresentationOfImage:image
                                                     toURL:destination_url
                                                 colorSpace:color_space
                                                    options:options
                                                      error:&error];
    CGColorSpaceRelease(color_space);
    return written ? 0 : 5;
    } @catch (NSException *exception) {
      return 6;
    }
  }
}
