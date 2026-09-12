/* Bounded IDCT-scaled JPEG decode. All libjpeg longjmps stay in this C frame. */
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <setjmp.h>
#include "jpeglib.h"

typedef int (*oxy_jpeg_cancel)(void *);
typedef struct {
    struct jpeg_error_mgr base;
    jmp_buf jump;
    char *message;
    size_t message_size;
    struct jpeg_progress_mgr progress;
    void *context;
    oxy_jpeg_cancel cancelled;
    int status;
} oxy_jpeg_error;

static void decode_error(j_common_ptr jpeg) {
    oxy_jpeg_error *error = (oxy_jpeg_error *)jpeg->err;
    char message[JMSG_LENGTH_MAX];
    (*jpeg->err->format_message)(jpeg, message);
    snprintf(error->message, error->message_size, "%s", message);
    longjmp(error->jump, 1);
}

static void decode_warning(j_common_ptr jpeg, int level) {
    if (level < 0) decode_error(jpeg);
}

static void decode_progress(j_common_ptr jpeg) {
    oxy_jpeg_error *error = (oxy_jpeg_error *)jpeg->err;
    if (error->cancelled(error->context)) {
        error->status = 2;
        longjmp(error->jump, 1);
    }
}

int oxy_libjpeg_decode_scaled(const unsigned char *input, unsigned long length,
                       unsigned char *output, size_t capacity,
                       uint32_t expected_width, uint32_t expected_height,
                       uint32_t denominator, uint32_t *width, uint32_t *height,
                       void *context, oxy_jpeg_cancel cancelled,
                       char *message, size_t message_size) {
    /* Heap state remains defined after longjmp even when libjpeg changed it. */
    struct jpeg_decompress_struct *jpeg = calloc(1, sizeof(*jpeg));
    oxy_jpeg_error *error = calloc(1, sizeof(*error));
    if (!jpeg || !error) { free(jpeg); free(error); return 3; }
    jpeg->err = jpeg_std_error(&error->base);
    error->base.error_exit = decode_error;
    error->base.emit_message = decode_warning;
    error->message = message;
    error->message_size = message_size;
    error->status = 1;
    error->context = context;
    error->cancelled = cancelled;
    error->progress.progress_monitor = decode_progress;
    if (setjmp(error->jump)) {
        int status = error->status;
        jpeg_destroy_decompress(jpeg); free(error); free(jpeg); return status;
    }
    jpeg_create_decompress(jpeg);
    jpeg->progress = &error->progress;
    jpeg_mem_src(jpeg, input, length);
    jpeg_read_header(jpeg, TRUE);
    if (jpeg->image_width != expected_width || jpeg->image_height != expected_height ||
        jpeg->num_components > 3 || jpeg->data_precision != 8) {
        snprintf(message, message_size, "Unsupported JPEG dimensions, components or precision");
        jpeg_destroy_decompress(jpeg); free(error); free(jpeg); return 1;
    }
    jpeg->scale_num = 1;
    jpeg->scale_denom = denominator;
    jpeg->out_color_space = JCS_RGB;
    jpeg_calc_output_dimensions(jpeg);
    if ((uint64_t)jpeg->output_width * jpeg->output_height * 3 > capacity) {
        snprintf(message, message_size, "Scaled JPEG exceeds reserved output");
        jpeg_destroy_decompress(jpeg); free(error); free(jpeg); return 1;
    }
    if (cancelled(context)) { jpeg_destroy_decompress(jpeg); free(error); free(jpeg); return 2; }
    jpeg_start_decompress(jpeg);
    while (jpeg->output_scanline < jpeg->output_height) {
        if (cancelled(context)) { jpeg_destroy_decompress(jpeg); free(error); free(jpeg); return 2; }
        JSAMPROW row = output + (size_t)jpeg->output_scanline * jpeg->output_width * 3;
        jpeg_read_scanlines(jpeg, &row, 1);
    }
    *width = jpeg->output_width;
    *height = jpeg->output_height;
    jpeg_finish_decompress(jpeg);
    jpeg_destroy_decompress(jpeg); free(error); free(jpeg); return 0;
}
