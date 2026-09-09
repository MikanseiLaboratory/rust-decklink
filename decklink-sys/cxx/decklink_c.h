#ifndef RUST_DECKLINK_C_H
#define RUST_DECKLINK_C_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef int32_t rdl_hresult;
typedef void *rdl_handle;

typedef void (*rdl_input_frame_fn)(void *ctx, rdl_handle video, rdl_handle audio);
typedef void (*rdl_input_format_fn)(
    void *ctx,
    uint32_t events,
    uint32_t display_mode,
    int32_t width,
    int32_t height,
    int64_t frame_duration,
    int64_t time_scale,
    uint32_t detected_flags);
typedef void (*rdl_output_completed_fn)(void *ctx, rdl_handle frame, uint32_t result);
typedef void (*rdl_output_stopped_fn)(void *ctx);
typedef void (*rdl_audio_render_fn)(void *ctx, int32_t preroll);

typedef struct rdl_input_callbacks {
    void *ctx;
    rdl_input_frame_fn frame;
    rdl_input_format_fn format;
} rdl_input_callbacks;

typedef struct rdl_output_callbacks {
    void *ctx;
    rdl_output_completed_fn completed;
    rdl_output_stopped_fn stopped;
    rdl_audio_render_fn audio_render;
} rdl_output_callbacks;

typedef struct rdl_video_info_t {
    int32_t width;
    int32_t height;
    int32_t row_bytes;
    uint32_t pixel_format;
    uint32_t flags;
    int64_t stream_time;
    int64_t stream_duration;
    int64_t hardware_time;
    int64_t hardware_duration;
} rdl_video_info_t;

typedef struct rdl_audio_info_t {
    int32_t sample_frame_count;
    int64_t packet_time;
} rdl_audio_info_t;

typedef struct rdl_display_mode_info_t {
    uint32_t mode;
    int32_t width;
    int32_t height;
    int64_t frame_duration;
    int64_t time_scale;
    uint32_t field_dominance;
    uint32_t flags;
    char name[128];
} rdl_display_mode_info_t;

rdl_hresult rdl_initialize(void);
void rdl_uninitialize(void);
int32_t rdl_hardware_available(void);
rdl_hresult rdl_api_version(char *buf, size_t len);

rdl_hresult rdl_iterator_create(rdl_handle *out);
rdl_hresult rdl_iterator_next(rdl_handle iterator, rdl_handle *device);

void rdl_add_ref(rdl_handle handle);
void rdl_release(rdl_handle handle);

rdl_hresult rdl_device_model_name(rdl_handle device, char *buf, size_t len);
rdl_hresult rdl_device_display_name(rdl_handle device, char *buf, size_t len);
rdl_hresult rdl_device_profile_flag(rdl_handle device, uint32_t attr_id, int32_t *value);
rdl_hresult rdl_device_profile_int(rdl_handle device, uint32_t attr_id, int64_t *value);
rdl_hresult rdl_device_query_input(rdl_handle device, rdl_handle *out);
rdl_hresult rdl_device_query_output(rdl_handle device, rdl_handle *out);

rdl_hresult rdl_input_display_mode_iterator(rdl_handle input, rdl_handle *out);
rdl_hresult rdl_output_display_mode_iterator(rdl_handle output, rdl_handle *out);
rdl_hresult rdl_display_mode_iterator_next(rdl_handle iterator, rdl_handle *mode);
rdl_hresult rdl_display_mode_info(rdl_handle mode, rdl_display_mode_info_t *info);

rdl_hresult rdl_input_does_support(
    rdl_handle input,
    uint32_t connection,
    uint32_t mode,
    uint32_t pixel_format,
    uint32_t conversion,
    uint32_t flags,
    uint32_t *actual_mode,
    int32_t *supported);
rdl_hresult rdl_input_set_callback(rdl_handle input, const rdl_input_callbacks *callbacks);
rdl_hresult rdl_input_enable_video(rdl_handle input, uint32_t mode, uint32_t pixel_format, uint32_t flags);
rdl_hresult rdl_input_disable_video(rdl_handle input);
rdl_hresult rdl_input_enable_audio(rdl_handle input, uint32_t sample_rate, uint32_t sample_type, uint32_t channels);
rdl_hresult rdl_input_disable_audio(rdl_handle input);
rdl_hresult rdl_input_start(rdl_handle input);
rdl_hresult rdl_input_stop(rdl_handle input);
rdl_hresult rdl_input_pause(rdl_handle input);
rdl_hresult rdl_input_flush(rdl_handle input);
rdl_hresult rdl_input_available_video_frames(rdl_handle input, uint32_t *count);

rdl_hresult rdl_video_info(rdl_handle frame, int64_t time_scale, rdl_video_info_t *info);
rdl_hresult rdl_video_map_read(rdl_handle frame, const uint8_t **data, size_t *len);
rdl_hresult rdl_video_unmap_read(rdl_handle frame);
rdl_hresult rdl_audio_info(rdl_handle packet, int64_t time_scale, rdl_audio_info_t *info);
rdl_hresult rdl_audio_bytes(rdl_handle packet, const uint8_t **data, size_t *len);

rdl_hresult rdl_output_does_support(
    rdl_handle output,
    uint32_t connection,
    uint32_t mode,
    uint32_t pixel_format,
    uint32_t conversion,
    uint32_t flags,
    uint32_t *actual_mode,
    int32_t *supported);
rdl_hresult rdl_output_set_callbacks(rdl_handle output, const rdl_output_callbacks *callbacks);
rdl_hresult rdl_output_enable_video(rdl_handle output, uint32_t mode, uint32_t flags);
rdl_hresult rdl_output_disable_video(rdl_handle output);
rdl_hresult rdl_output_enable_audio(
    rdl_handle output,
    uint32_t sample_rate,
    uint32_t sample_type,
    uint32_t channels,
    uint32_t stream_type);
rdl_hresult rdl_output_disable_audio(rdl_handle output);
rdl_hresult rdl_output_row_bytes(rdl_handle output, uint32_t pixel_format, int32_t width, int32_t *row_bytes);
rdl_hresult rdl_output_create_frame(
    rdl_handle output,
    int32_t width,
    int32_t height,
    int32_t row_bytes,
    uint32_t pixel_format,
    uint32_t flags,
    const uint8_t *data,
    size_t data_len,
    rdl_handle *frame);
rdl_hresult rdl_output_schedule_video(
    rdl_handle output,
    rdl_handle frame,
    int64_t display_time,
    int64_t display_duration,
    int64_t time_scale);
rdl_hresult rdl_output_schedule_audio(
    rdl_handle output,
    const uint8_t *data,
    uint32_t sample_frame_count,
    int64_t stream_time,
    int64_t time_scale,
    uint32_t *written);
rdl_hresult rdl_output_begin_audio_preroll(rdl_handle output);
rdl_hresult rdl_output_end_audio_preroll(rdl_handle output);
rdl_hresult rdl_output_start(rdl_handle output, int64_t start_time, int64_t time_scale, double speed);
rdl_hresult rdl_output_stop(rdl_handle output, int64_t stop_time, int64_t time_scale, int64_t *actual);
rdl_hresult rdl_output_buffered_video(rdl_handle output, uint32_t *count);
rdl_hresult rdl_output_buffered_audio(rdl_handle output, uint32_t *count);
rdl_hresult rdl_output_flush_audio(rdl_handle output);

#ifdef __cplusplus
}
#endif

#endif
