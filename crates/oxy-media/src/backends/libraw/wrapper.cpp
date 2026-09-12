#include "libraw/libraw.h"

extern "C" int oxy_libraw_unpack_sized_thumb(libraw_data_t *raw,
                                               unsigned target_size) {
  // Prefer the cheapest preview that does not need upscaling.
  int selected = -1;
  unsigned long long selected_size = 0;
  bool selected_is_large_enough = false;

  for (int index = 0; index < raw->thumbs_list.thumbcount; ++index) {
    const libraw_thumbnail_item_t &thumbnail =
        raw->thumbs_list.thumblist[index];
    // Zero requests the largest JPEG, independently of display target sizes.
    if (target_size == 0) {
      if (thumbnail.tformat == LIBRAW_INTERNAL_THUMBNAIL_JPEG &&
          (selected < 0 ||
           static_cast<unsigned long long>(thumbnail.twidth) * thumbnail.theight >
               selected_size)) {
        selected = index;
        selected_size = static_cast<unsigned long long>(thumbnail.twidth) * thumbnail.theight;
      }
      continue;
    }
    if (thumbnail.tformat == LIBRAW_INTERNAL_THUMBNAIL_JPEGXL) {
      continue;
    }

    const unsigned size =
        thumbnail.twidth > thumbnail.theight ? thumbnail.twidth
                                             : thumbnail.theight;
    if (size == 0) {
      continue;
    }

    const bool is_large_enough = size >= target_size;
    if (selected < 0 ||
        (is_large_enough && !selected_is_large_enough) ||
        (is_large_enough == selected_is_large_enough &&
         ((is_large_enough && size < selected_size) ||
          (!is_large_enough && size > selected_size)))) {
      selected = index;
      selected_size = size;
      selected_is_large_enough = is_large_enough;
    }
  }

  if (target_size == 0 && selected < 0) {
    return LIBRAW_NO_THUMBNAIL;
  }
  return selected >= 0 ? libraw_unpack_thumb_ex(raw, selected)
                       : libraw_unpack_thumb(raw);
}

extern "C" void oxy_libraw_configure_preview(libraw_data_t *raw) {
  raw->params.half_size = 1;
  raw->params.use_camera_wb = 1;
  raw->params.use_camera_matrix = 3;
  raw->params.output_color = 1;
  raw->params.output_bps = 8;
}

extern "C" void oxy_libraw_configure_full(libraw_data_t *raw) {
  raw->params.half_size = 0;
  raw->params.use_camera_wb = 1;
  raw->params.use_camera_matrix = 3;
  raw->params.output_color = 1;
  raw->params.output_bps = 8;
  raw->params.four_color_rgb = 1;
  raw->params.user_qual = 3;
  raw->params.fbdd_noiserd = 0;
}
