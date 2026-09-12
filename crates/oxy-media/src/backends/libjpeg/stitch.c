/* OxyViewer JPEG coefficient stitcher. libjpeg errors never cross a Rust frame. */
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <time.h>
#include "jpeglib.h"
#include "jerror.h"
#ifdef _WIN32
#include <windows.h>
#endif

typedef struct { const unsigned char *data; size_t length; uint32_t x, y, width, height; } oxy_tile;
typedef int (*oxy_cancel)(void *);
typedef int (*oxy_write)(void *, const unsigned char *, size_t);
typedef struct { double read_ms, copy_ms, write_ms; uint64_t coefficient_bytes; } oxy_stats;
typedef struct oxy_state oxy_state;
typedef struct { struct jpeg_error_mgr base; oxy_state *state; } oxy_error;
typedef struct { struct jpeg_progress_mgr base; oxy_state *state; } oxy_progress;
typedef struct { struct jpeg_destination_mgr base; oxy_state *state; unsigned char buffer[65536]; } oxy_destination;
struct oxy_state {
    struct jpeg_decompress_struct input;
    struct jpeg_compress_struct output;
    oxy_error in_error, out_error;
    oxy_progress progress;
    oxy_destination destination;
    jmp_buf jump;
    int status;
    char *message;
    size_t message_size;
    void *context;
    oxy_cancel cancelled;
    oxy_write write;
};
static double oxy_now(void) {
#ifdef _WIN32
    LARGE_INTEGER n, f; QueryPerformanceCounter(&n); QueryPerformanceFrequency(&f);
    return 1000.0 * (double)n.QuadPart / (double)f.QuadPart;
#else
    struct timespec n; clock_gettime(CLOCK_MONOTONIC, &n);
    return 1000.0 * (double)n.tv_sec + (double)n.tv_nsec / 1000000.0;
#endif
}
static void oxy_fail(oxy_state *s, int status, const char *message) {
    s->status = status;
    if (s->message_size) snprintf(s->message, s->message_size, "%s", message);
    longjmp(s->jump, 1);
}
static void oxy_check(oxy_state *s) {
    /* The callback returns before any longjmp. */
    if (s->cancelled(s->context)) oxy_fail(s, 2, "JPEG stitch cancelled");
}
static void oxy_error_exit(j_common_ptr jpeg) {
    oxy_error *error = (oxy_error *)jpeg->err;
    char message[JMSG_LENGTH_MAX];
    (*jpeg->err->format_message)(jpeg, message);
    oxy_fail(error->state, jpeg->err->msg_code == JERR_OUT_OF_MEMORY ? 3 : 4, message);
}
static void oxy_emit(j_common_ptr jpeg, int level) {
    /* Truncated/corrupt streams must not become valid cache artifacts. */
    if (level < 0) oxy_error_exit(jpeg);
}
static void oxy_progress_check(j_common_ptr jpeg) {
    oxy_progress *p = (oxy_progress *)jpeg->progress;
    oxy_check(p->state);
}
static void oxy_destination_init(j_compress_ptr jpeg) {
    oxy_destination *d = (oxy_destination *)jpeg->dest;
    d->base.next_output_byte = d->buffer;
    d->base.free_in_buffer = sizeof(d->buffer);
}
static void oxy_flush(oxy_destination *d, size_t length) {
    oxy_check(d->state);
    if (length && d->state->write(d->state->context, d->buffer, length))
        oxy_fail(d->state, 3, "JPEG artifact write failed");
}
static boolean oxy_destination_empty(j_compress_ptr jpeg) {
    oxy_destination *d = (oxy_destination *)jpeg->dest;
    oxy_flush(d, sizeof(d->buffer)); oxy_destination_init(jpeg); return TRUE;
}
static void oxy_destination_finish(j_compress_ptr jpeg) {
    oxy_destination *d = (oxy_destination *)jpeg->dest;
    oxy_flush(d, sizeof(d->buffer) - d->base.free_in_buffer);
}
static void oxy_open(oxy_state *s, const oxy_tile *tile) {
    oxy_check(s);
    memset(&s->input, 0, sizeof(s->input));
    s->input.err = jpeg_std_error(&s->in_error.base);
    s->in_error.state = s;
    s->in_error.base.error_exit = oxy_error_exit;
    s->in_error.base.emit_message = oxy_emit;
    jpeg_create_decompress(&s->input);
    s->input.progress = &s->progress.base;
    if (!tile->data || !tile->length || tile->length > 0xFFFFFFFFUL)
        oxy_fail(s, 1, "invalid JPEG tile byte length");
    jpeg_mem_src(&s->input, tile->data, (unsigned long)tile->length);
    /* ICC/EXIF need explicit metadata handling; leave these to the pixel fallback. */
    jpeg_save_markers(&s->input, JPEG_APP0 + 1, 1);
    jpeg_save_markers(&s->input, JPEG_APP0 + 2, 1);
    jpeg_read_header(&s->input, TRUE);
    if (s->input.marker_list || s->input.data_precision != 8 || s->input.num_components != 3 ||
        s->input.jpeg_color_space != JCS_YCbCr || s->input.progressive_mode || s->input.arith_code)
        oxy_fail(s, 1, "unsupported JPEG metadata, color, precision or coding");
    /* Chroma upsampling across a newly joined tile boundary can change pixels.
       Support actual 4:4:4 only, including FFmpeg's equal 1x2 sampling factors. */
    for (int c = 1; c < 3; c++) {
        if (s->input.comp_info[c].h_samp_factor != s->input.comp_info[0].h_samp_factor ||
            s->input.comp_info[c].v_samp_factor != s->input.comp_info[0].v_samp_factor)
            oxy_fail(s, 1, "subsampled JPEG requires pixel fallback");
    }
    if (s->input.image_width != tile->width || s->input.image_height != tile->height)
        oxy_fail(s, 4, "JPEG tile dimensions differ from publication");
}
static void oxy_compatible(oxy_state *s) {
    for (int c = 0; c < 3; c++) {
        jpeg_component_info *a = &s->input.comp_info[c], *b = &s->output.comp_info[c];
        if (a->h_samp_factor != b->h_samp_factor || a->v_samp_factor != b->v_samp_factor ||
            a->component_id != b->component_id)
            oxy_fail(s, 1, "JPEG sampling/components mismatch");
        JQUANT_TBL *qa = s->input.quant_tbl_ptrs[a->quant_tbl_no];
        JQUANT_TBL *qb = s->output.quant_tbl_ptrs[b->quant_tbl_no];
        if (!qa || !qb || memcmp(qa->quantval, qb->quantval, sizeof(qa->quantval)))
            oxy_fail(s, 1, "JPEG quantization mismatch");
    }
}
static uint64_t oxy_coeff_bytes(oxy_state *s, uint32_t w, uint32_t h, int mh, int mv) {
    uint64_t mcus = ((uint64_t)w + mh * 8 - 1) / (mh * 8) * (((uint64_t)h + mv * 8 - 1) / (mv * 8));
    uint64_t blocks = 0;
    for (int c = 0; c < 3; c++) blocks += s->output.comp_info[c].h_samp_factor * s->output.comp_info[c].v_samp_factor;
    return mcus * blocks * sizeof(JBLOCK);
}
/* Status: 0 success, 1 unsupported (safe fallback), 2 cancelled,
   3 I/O/allocation failure, 4 corrupt/incomplete input (never publish). */
int oxy_libjpeg_stitch_coefficients(const oxy_tile *tiles, size_t count, uint32_t width, uint32_t height,
                    uint64_t memory_budget, void *context, oxy_cancel cancel, oxy_write write,
                    oxy_stats *stats, char *message, size_t message_size) {
    oxy_state *s = (oxy_state *)calloc(1, sizeof(*s));
    if (!s) return 3;
    s->context = context; s->cancelled = cancel; s->write = write;
    s->message = message; s->message_size = message_size;
    s->progress.state = s; s->progress.base.progress_monitor = oxy_progress_check;
    /* All mutable cleanup state lives on the heap, avoiding setjmp-clobbered locals. */
    if (setjmp(s->jump)) {
        int status = s->status;
        if (s->input.mem) jpeg_destroy_decompress(&s->input);
        if (s->output.mem) jpeg_destroy_compress(&s->output);
        free(s); return status;
    }
    if (!tiles || !count || count > 256 || !width || !height || width > JPEG_MAX_DIMENSION || height > JPEG_MAX_DIMENSION)
        oxy_fail(s, 1, "invalid JPEG canvas/tile count");
    s->output.err = jpeg_std_error(&s->out_error.base);
    s->out_error.state = s;
    s->out_error.base.error_exit = oxy_error_exit;
    s->out_error.base.emit_message = oxy_emit;
    jpeg_create_compress(&s->output);
    s->output.progress = &s->progress.base;
    oxy_open(s, &tiles[0]);
    jpeg_copy_critical_parameters(&s->input, &s->output);
    jpeg_destroy_decompress(&s->input);
    s->output.image_width = width; s->output.image_height = height;
    s->output.optimize_coding = FALSE;
    int mh = 0, mv = 0;
    for (int c = 0; c < 3; c++) {
        if (s->output.comp_info[c].h_samp_factor > mh) mh = s->output.comp_info[c].h_samp_factor;
        if (s->output.comp_info[c].v_samp_factor > mv) mv = s->output.comp_info[c].v_samp_factor;
    }
    uint64_t area = 0, max_input = 0;
    for (size_t t = 0; t < count; t++) {
        const oxy_tile *a = &tiles[t];
        oxy_open(s, a); oxy_compatible(s);
        if (!a->width || !a->height || a->x > width || a->y > height ||
            a->width > width - a->x || a->height > height - a->y || a->x % (mh * 8) || a->y % (mv * 8) ||
            (a->x + a->width < width && a->width % (mh * 8)) ||
            (a->y + a->height < height && a->height % (mv * 8))) oxy_fail(s, 1, "JPEG tile bounds/alignment mismatch");
        for (size_t j = 0; j < t; j++) {
            const oxy_tile *b = &tiles[j];
            if (a->x < b->x + b->width && a->x + a->width > b->x &&
                a->y < b->y + b->height && a->y + a->height > b->y) oxy_fail(s, 4, "JPEG tiles overlap");
        }
        area += (uint64_t)a->width * a->height;
        uint64_t bytes = oxy_coeff_bytes(s, a->width, a->height, mh, mv);
        if (bytes > max_input) max_input = bytes;
        jpeg_destroy_decompress(&s->input);
    }
    if (area != (uint64_t)width * height) oxy_fail(s, 4, "JPEG tiles do not cover canvas");
    stats->coefficient_bytes = oxy_coeff_bytes(s, width, height, mh, mv) + max_input;
    if (stats->coefficient_bytes + 8 * 1024 * 1024 > memory_budget)
        oxy_fail(s, 1, "JPEG coefficient memory budget exceeded");
    s->destination.state = s;
    s->destination.base.init_destination = oxy_destination_init;
    s->destination.base.empty_output_buffer = oxy_destination_empty;
    s->destination.base.term_destination = oxy_destination_finish;
    s->output.dest = &s->destination.base;
    jvirt_barray_ptr dst[3];
    for (int c = 0; c < 3; c++) {
        int hs = s->output.comp_info[c].h_samp_factor, vs = s->output.comp_info[c].v_samp_factor;
        JDIMENSION bw = ((width + mh * 8 - 1) / (mh * 8)) * hs;
        JDIMENSION bh = ((height + mv * 8 - 1) / (mv * 8)) * vs;
        dst[c] = s->output.mem->request_virt_barray((j_common_ptr)&s->output, JPOOL_IMAGE, TRUE, bw, bh, vs);
    }
    jpeg_write_coefficients(&s->output, dst);
    /* Virtual arrays require first writes in row order even with pre_zero.
       Initialize rows before visiting arbitrary display-order tiles. */
    for (int c = 0; c < 3; c++) {
        JDIMENSION rows = ((height + mv * 8 - 1) / (mv * 8)) * s->output.comp_info[c].v_samp_factor;
        for (JDIMENSION row = 0; row < rows; row++) {
            oxy_check(s);
            (void)s->output.mem->access_virt_barray((j_common_ptr)&s->output, dst[c], row, 1, TRUE);
        }
    }
    for (size_t t = 0; t < count; t++) {
        double started = oxy_now(); oxy_open(s, &tiles[t]);
        jvirt_barray_ptr *src = jpeg_read_coefficients(&s->input);
        oxy_compatible(s); oxy_check(s);
        stats->read_ms += oxy_now() - started;
        started = oxy_now();
        for (int c = 0; c < 3; c++) {
            jpeg_component_info *co = &s->input.comp_info[c];
            JDIMENSION dx = tiles[t].x / (mh * 8) * co->h_samp_factor;
            JDIMENSION dy = tiles[t].y / (mv * 8) * co->v_samp_factor;
            for (JDIMENSION row = 0; row < co->height_in_blocks; row++) {
                oxy_check(s);
                JBLOCKARRAY a = s->input.mem->access_virt_barray((j_common_ptr)&s->input, src[c], row, 1, FALSE);
                JBLOCKARRAY b = s->output.mem->access_virt_barray((j_common_ptr)&s->output, dst[c], dy + row, 1, TRUE);
                memcpy(b[0] + dx, a[0], co->width_in_blocks * sizeof(JBLOCK));
            }
        }
        stats->copy_ms += oxy_now() - started;
        jpeg_finish_decompress(&s->input); jpeg_destroy_decompress(&s->input);
    }
    double started = oxy_now(); oxy_check(s); jpeg_finish_compress(&s->output);
    stats->write_ms += oxy_now() - started;
    jpeg_destroy_compress(&s->output); free(s); return 0;
}
