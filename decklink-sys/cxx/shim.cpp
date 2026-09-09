#include "decklink_c.h"

#include <atomic>
#include <cstring>
#include <new>
#include <string>

#ifdef RDL_HARDWARE
#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <comdef.h>
#include <objbase.h>
#include "DeckLinkAPI_h.h"
#else
#include "DeckLinkAPI.h"
#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#endif
#endif
#endif

namespace {

constexpr rdl_hresult kOk = 0;
constexpr rdl_hresult kFalse = 1;
constexpr rdl_hresult kNotImpl = static_cast<rdl_hresult>(0x80000001);
constexpr rdl_hresult kInvalidArg = static_cast<rdl_hresult>(0x80000003);
constexpr rdl_hresult kNoInterface = static_cast<rdl_hresult>(0x80000004);
constexpr rdl_hresult kFail = static_cast<rdl_hresult>(0x80000008);

void copy_cstr(char *buf, size_t len, const std::string &value)
{
    if (!buf || len == 0) {
        return;
    }
    const size_t n = value.size() < len - 1 ? value.size() : len - 1;
    std::memcpy(buf, value.data(), n);
    buf[n] = '\0';
}

#ifdef RDL_HARDWARE

#ifdef _WIN32
std::atomic<int> g_com_init{0};
#endif

IUnknown *as_unknown(rdl_handle handle)
{
    return static_cast<IUnknown *>(handle);
}

template <typename T>
T *as(rdl_handle handle)
{
    return static_cast<T *>(handle);
}

bool iid_eq(REFIID lhs, REFIID rhs)
{
#ifdef _WIN32
    return IsEqualGUID(lhs, rhs) == TRUE;
#else
    return std::memcmp(&lhs, &rhs, sizeof(REFIID)) == 0;
#endif
}

rdl_hresult copy_sdk_string(char *buf, size_t len, HRESULT hr
#ifdef _WIN32
    , BSTR value
#elif defined(__APPLE__)
    , CFStringRef value
#else
    , const char *value
#endif
)
{
    if (hr != S_OK) {
        return static_cast<rdl_hresult>(hr);
    }
#ifdef _WIN32
    if (!value) {
        copy_cstr(buf, len, std::string());
        return kOk;
    }
    const int needed = WideCharToMultiByte(CP_UTF8, 0, value, -1, nullptr, 0, nullptr, nullptr);
    std::string utf8;
    if (needed > 0) {
        utf8.resize(static_cast<size_t>(needed - 1));
        WideCharToMultiByte(CP_UTF8, 0, value, -1, utf8.data(), needed, nullptr, nullptr);
    }
    SysFreeString(value);
    copy_cstr(buf, len, utf8);
#elif defined(__APPLE__)
    if (!value) {
        copy_cstr(buf, len, std::string());
        return kOk;
    }
    const CFIndex max_size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), kCFStringEncodingUTF8) + 1;
    std::string utf8(static_cast<size_t>(max_size), '\0');
    if (CFStringGetCString(value, utf8.data(), max_size, kCFStringEncodingUTF8)) {
        utf8.resize(std::strlen(utf8.c_str()));
    } else {
        utf8.clear();
    }
    CFRelease(value);
    copy_cstr(buf, len, utf8);
#else
    if (!value) {
        copy_cstr(buf, len, std::string());
        return kOk;
    }
    copy_cstr(buf, len, std::string(value));
    free(const_cast<char *>(value));
#endif
    return kOk;
}

class InputCallback final : public IDeckLinkInputCallback
{
public:
    explicit InputCallback(rdl_input_callbacks callbacks)
        : m_callbacks(callbacks)
        , m_refs(1)
    {
    }

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID *ppv) override
    {
        if (!ppv) {
            return E_POINTER;
        }
        if (iid_eq(iid, IID_IUnknown) || iid_eq(iid, IID_IDeckLinkInputCallback)) {
            *ppv = static_cast<IDeckLinkInputCallback *>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    ULONG STDMETHODCALLTYPE AddRef() override
    {
        return ++m_refs;
    }

    ULONG STDMETHODCALLTYPE Release() override
    {
        const ULONG value = --m_refs;
        if (value == 0) {
            delete this;
        }
        return value;
    }

    HRESULT STDMETHODCALLTYPE VideoInputFormatChanged(
        BMDVideoInputFormatChangedEvents events,
        IDeckLinkDisplayMode *mode,
        BMDDetectedVideoInputFormatFlags flags) override
    {
        if (!m_callbacks.format) {
            return S_OK;
        }
        rdl_display_mode_info info{};
        if (mode) {
            fill_display_mode(mode, &info);
        }
        m_callbacks.format(
            m_callbacks.ctx,
            events,
            info.mode,
            info.width,
            info.height,
            info.frame_duration,
            info.time_scale,
            flags);
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE VideoInputFrameArrived(
        IDeckLinkVideoInputFrame *video,
        IDeckLinkAudioInputPacket *audio) override
    {
        if (!m_callbacks.frame) {
            return S_OK;
        }
        if (video) {
            video->AddRef();
        }
        if (audio) {
            audio->AddRef();
        }
        m_callbacks.frame(m_callbacks.ctx, video, audio);
        return S_OK;
    }

    static void fill_display_mode(IDeckLinkDisplayMode *mode, rdl_display_mode_info_t *info);

private:
    rdl_input_callbacks m_callbacks;
    std::atomic<ULONG> m_refs;
};

class OutputCallback final : public IDeckLinkVideoOutputCallback, public IDeckLinkAudioOutputCallback
{
public:
    explicit OutputCallback(rdl_output_callbacks callbacks)
        : m_callbacks(callbacks)
        , m_refs(1)
    {
    }

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID *ppv) override
    {
        if (!ppv) {
            return E_POINTER;
        }
        if (iid_eq(iid, IID_IUnknown) || iid_eq(iid, IID_IDeckLinkVideoOutputCallback)) {
            *ppv = static_cast<IDeckLinkVideoOutputCallback *>(this);
            AddRef();
            return S_OK;
        }
        if (iid_eq(iid, IID_IDeckLinkAudioOutputCallback)) {
            *ppv = static_cast<IDeckLinkAudioOutputCallback *>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    ULONG STDMETHODCALLTYPE AddRef() override
    {
        return ++m_refs;
    }

    ULONG STDMETHODCALLTYPE Release() override
    {
        const ULONG value = --m_refs;
        if (value == 0) {
            delete this;
        }
        return value;
    }

    HRESULT STDMETHODCALLTYPE ScheduledFrameCompleted(
        IDeckLinkVideoFrame *completedFrame,
        BMDOutputFrameCompletionResult result) override
    {
        if (m_callbacks.completed) {
            if (completedFrame) {
                completedFrame->AddRef();
            }
            m_callbacks.completed(m_callbacks.ctx, completedFrame, result);
        }
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE ScheduledPlaybackHasStopped() override
    {
        if (m_callbacks.stopped) {
            m_callbacks.stopped(m_callbacks.ctx);
        }
        return S_OK;
    }

#ifdef _WIN32
    HRESULT STDMETHODCALLTYPE RenderAudioSamples(BOOL preroll) override
#else
    HRESULT STDMETHODCALLTYPE RenderAudioSamples(bool preroll) override
#endif
    {
        if (m_callbacks.audio_render) {
#ifdef _WIN32
            m_callbacks.audio_render(m_callbacks.ctx, preroll ? 1 : 0);
#else
            m_callbacks.audio_render(m_callbacks.ctx, preroll ? 1 : 0);
#endif
        }
        return S_OK;
    }

private:
    rdl_output_callbacks m_callbacks;
    std::atomic<ULONG> m_refs;
};

void InputCallback::fill_display_mode(IDeckLinkDisplayMode *mode, rdl_display_mode_info_t *info)
{
    if (!mode || !info) {
        return;
    }
    info->mode = mode->GetDisplayMode();
    info->width = static_cast<int32_t>(mode->GetWidth());
    info->height = static_cast<int32_t>(mode->GetHeight());
    BMDTimeValue duration = 0;
    BMDTimeScale scale = 0;
    mode->GetFrameRate(&duration, &scale);
    info->frame_duration = duration;
    info->time_scale = scale;
    info->field_dominance = mode->GetFieldDominance();
    info->flags = mode->GetFlags();
#ifdef _WIN32
    BSTR name = nullptr;
    copy_sdk_string(info->name, sizeof(info->name), mode->GetName(&name), name);
#elif defined(__APPLE__)
    CFStringRef name = nullptr;
    copy_sdk_string(info->name, sizeof(info->name), mode->GetName(&name), name);
#else
    const char *name = nullptr;
    copy_sdk_string(info->name, sizeof(info->name), mode->GetName(&name), name);
#endif
}

rdl_hresult query_profile(rdl_handle device, IDeckLinkProfileAttributes **out)
{
    if (!device || !out) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLink>(device)->QueryInterface(
        IID_IDeckLinkProfileAttributes, reinterpret_cast<void **>(out)));
}

#endif // RDL_HARDWARE

} // namespace

#ifdef RDL_STUB

extern "C" {

rdl_hresult rdl_initialize(void) { return kNotImpl; }
void rdl_uninitialize(void) {}
int32_t rdl_hardware_available(void) { return 0; }
rdl_hresult rdl_api_version(char *buf, size_t len)
{
    copy_cstr(buf, len, "stub");
    return kNotImpl;
}
rdl_hresult rdl_iterator_create(rdl_handle *) { return kNotImpl; }
rdl_hresult rdl_iterator_next(rdl_handle, rdl_handle *) { return kNotImpl; }
void rdl_add_ref(rdl_handle) {}
void rdl_release(rdl_handle) {}
rdl_hresult rdl_device_model_name(rdl_handle, char *, size_t) { return kNotImpl; }
rdl_hresult rdl_device_display_name(rdl_handle, char *, size_t) { return kNotImpl; }
rdl_hresult rdl_device_profile_flag(rdl_handle, uint32_t, int32_t *) { return kNotImpl; }
rdl_hresult rdl_device_profile_int(rdl_handle, uint32_t, int64_t *) { return kNotImpl; }
rdl_hresult rdl_device_query_input(rdl_handle, rdl_handle *) { return kNotImpl; }
rdl_hresult rdl_device_query_output(rdl_handle, rdl_handle *) { return kNotImpl; }
rdl_hresult rdl_input_display_mode_iterator(rdl_handle, rdl_handle *) { return kNotImpl; }
rdl_hresult rdl_output_display_mode_iterator(rdl_handle, rdl_handle *) { return kNotImpl; }
rdl_hresult rdl_display_mode_iterator_next(rdl_handle, rdl_handle *) { return kNotImpl; }
rdl_hresult rdl_display_mode_info(rdl_handle, rdl_display_mode_info_t *) { return kNotImpl; }
rdl_hresult rdl_input_does_support(rdl_handle, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *, int32_t *)
{
    return kNotImpl;
}
rdl_hresult rdl_input_set_callback(rdl_handle, const rdl_input_callbacks *) { return kNotImpl; }
rdl_hresult rdl_input_enable_video(rdl_handle, uint32_t, uint32_t, uint32_t) { return kNotImpl; }
rdl_hresult rdl_input_disable_video(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_input_enable_audio(rdl_handle, uint32_t, uint32_t, uint32_t) { return kNotImpl; }
rdl_hresult rdl_input_disable_audio(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_input_start(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_input_stop(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_input_pause(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_input_flush(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_input_available_video_frames(rdl_handle, uint32_t *) { return kNotImpl; }
rdl_hresult rdl_video_info(rdl_handle, int64_t, rdl_video_info_t *) { return kNotImpl; }
rdl_hresult rdl_video_map_read(rdl_handle, const uint8_t **, size_t *) { return kNotImpl; }
rdl_hresult rdl_video_unmap_read(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_audio_info(rdl_handle, int64_t, rdl_audio_info_t *) { return kNotImpl; }
rdl_hresult rdl_audio_bytes(rdl_handle, const uint8_t **, size_t *) { return kNotImpl; }
rdl_hresult rdl_output_does_support(rdl_handle, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *, int32_t *)
{
    return kNotImpl;
}
rdl_hresult rdl_output_set_callbacks(rdl_handle, const rdl_output_callbacks *) { return kNotImpl; }
rdl_hresult rdl_output_enable_video(rdl_handle, uint32_t, uint32_t) { return kNotImpl; }
rdl_hresult rdl_output_disable_video(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_output_enable_audio(rdl_handle, uint32_t, uint32_t, uint32_t, uint32_t) { return kNotImpl; }
rdl_hresult rdl_output_disable_audio(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_output_row_bytes(rdl_handle, uint32_t, int32_t, int32_t *) { return kNotImpl; }
rdl_hresult rdl_output_create_frame(
    rdl_handle, int32_t, int32_t, int32_t, uint32_t, uint32_t, const uint8_t *, size_t, rdl_handle *)
{
    return kNotImpl;
}
rdl_hresult rdl_output_schedule_video(rdl_handle, rdl_handle, int64_t, int64_t, int64_t) { return kNotImpl; }
rdl_hresult rdl_output_schedule_audio(rdl_handle, const uint8_t *, uint32_t, int64_t, int64_t, uint32_t *)
{
    return kNotImpl;
}
rdl_hresult rdl_output_begin_audio_preroll(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_output_end_audio_preroll(rdl_handle) { return kNotImpl; }
rdl_hresult rdl_output_start(rdl_handle, int64_t, int64_t, double) { return kNotImpl; }
rdl_hresult rdl_output_stop(rdl_handle, int64_t, int64_t, int64_t *) { return kNotImpl; }
rdl_hresult rdl_output_buffered_video(rdl_handle, uint32_t *) { return kNotImpl; }
rdl_hresult rdl_output_buffered_audio(rdl_handle, uint32_t *) { return kNotImpl; }
rdl_hresult rdl_output_flush_audio(rdl_handle) { return kNotImpl; }

}

#else

extern "C" {

rdl_hresult rdl_initialize(void)
{
    try {
#ifdef _WIN32
        const HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        if (hr == S_OK) {
            g_com_init.fetch_add(1);
            return kOk;
        }
        if (hr == S_FALSE || hr == RPC_E_CHANGED_MODE) {
            return kOk;
        }
        return static_cast<rdl_hresult>(hr);
#else
        return kOk;
#endif
    } catch (...) {
        return kFail;
    }
}

void rdl_uninitialize(void)
{
#ifdef _WIN32
    if (g_com_init.fetch_sub(1) == 1) {
        CoUninitialize();
    }
#endif
}

int32_t rdl_hardware_available(void)
{
    return 1;
}

rdl_hresult rdl_api_version(char *buf, size_t len)
{
    try {
#ifdef _WIN32
        IDeckLinkAPIInformation *info = nullptr;
        HRESULT hr = CoCreateInstance(
            CLSID_CDeckLinkAPIInformation, nullptr, CLSCTX_ALL, IID_IDeckLinkAPIInformation, reinterpret_cast<void **>(&info));
        if (FAILED(hr) || !info) {
            copy_cstr(buf, len, "16.0");
            return kOk;
        }
        BSTR value = nullptr;
        hr = info->GetString(BMDDeckLinkAPIVersion, &value);
        info->Release();
        return copy_sdk_string(buf, len, hr, value);
#else
        IDeckLinkAPIInformation *info = CreateDeckLinkAPIInformationInstance();
        if (!info) {
            copy_cstr(buf, len, "16.0");
            return kOk;
        }
#ifdef __APPLE__
        CFStringRef value = nullptr;
#else
        const char *value = nullptr;
#endif
        const HRESULT hr = info->GetString(BMDDeckLinkAPIVersion, &value);
        info->Release();
        return copy_sdk_string(buf, len, hr, value);
#endif
    } catch (...) {
        return kFail;
    }
}

rdl_hresult rdl_iterator_create(rdl_handle *out)
{
    if (!out) {
        return kInvalidArg;
    }
    try {
#ifdef _WIN32
        IDeckLinkIterator *iterator = nullptr;
        const HRESULT hr = CoCreateInstance(
            CLSID_CDeckLinkIterator, nullptr, CLSCTX_ALL, IID_IDeckLinkIterator, reinterpret_cast<void **>(&iterator));
        *out = iterator;
        return static_cast<rdl_hresult>(hr);
#else
        IDeckLinkIterator *iterator = CreateDeckLinkIteratorInstance();
        *out = iterator;
        return iterator ? kOk : kFail;
#endif
    } catch (...) {
        return kFail;
    }
}

rdl_hresult rdl_iterator_next(rdl_handle iterator, rdl_handle *device)
{
    if (!iterator || !device) {
        return kInvalidArg;
    }
    IDeckLink *next = nullptr;
    const HRESULT hr = as<IDeckLinkIterator>(iterator)->Next(&next);
    *device = next;
    return static_cast<rdl_hresult>(hr);
}

void rdl_add_ref(rdl_handle handle)
{
    if (handle) {
        as_unknown(handle)->AddRef();
    }
}

void rdl_release(rdl_handle handle)
{
    if (handle) {
        as_unknown(handle)->Release();
    }
}

rdl_hresult rdl_device_model_name(rdl_handle device, char *buf, size_t len)
{
    if (!device) {
        return kInvalidArg;
    }
#ifdef _WIN32
    BSTR value = nullptr;
    return copy_sdk_string(buf, len, as<IDeckLink>(device)->GetModelName(&value), value);
#elif defined(__APPLE__)
    CFStringRef value = nullptr;
    return copy_sdk_string(buf, len, as<IDeckLink>(device)->GetModelName(&value), value);
#else
    const char *value = nullptr;
    return copy_sdk_string(buf, len, as<IDeckLink>(device)->GetModelName(&value), value);
#endif
}

rdl_hresult rdl_device_display_name(rdl_handle device, char *buf, size_t len)
{
    if (!device) {
        return kInvalidArg;
    }
#ifdef _WIN32
    BSTR value = nullptr;
    return copy_sdk_string(buf, len, as<IDeckLink>(device)->GetDisplayName(&value), value);
#elif defined(__APPLE__)
    CFStringRef value = nullptr;
    return copy_sdk_string(buf, len, as<IDeckLink>(device)->GetDisplayName(&value), value);
#else
    const char *value = nullptr;
    return copy_sdk_string(buf, len, as<IDeckLink>(device)->GetDisplayName(&value), value);
#endif
}

rdl_hresult rdl_device_profile_flag(rdl_handle device, uint32_t attr_id, int32_t *value)
{
    IDeckLinkProfileAttributes *attrs = nullptr;
    const rdl_hresult qi = query_profile(device, &attrs);
    if (qi != kOk || !attrs) {
        return qi == kOk ? kNoInterface : qi;
    }
#ifdef _WIN32
    BOOL flag = FALSE;
#else
    bool flag = false;
#endif
    const HRESULT hr = attrs->GetFlag(static_cast<BMDDeckLinkAttributeID>(attr_id), &flag);
    attrs->Release();
    if (value) {
        *value = flag ? 1 : 0;
    }
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_device_profile_int(rdl_handle device, uint32_t attr_id, int64_t *value)
{
    IDeckLinkProfileAttributes *attrs = nullptr;
    const rdl_hresult qi = query_profile(device, &attrs);
    if (qi != kOk || !attrs) {
        return qi == kOk ? kNoInterface : qi;
    }
    int64_t number = 0;
    const HRESULT hr = attrs->GetInt(static_cast<BMDDeckLinkAttributeID>(attr_id), &number);
    attrs->Release();
    if (value) {
        *value = number;
    }
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_device_query_input(rdl_handle device, rdl_handle *out)
{
    if (!device || !out) {
        return kInvalidArg;
    }
    IDeckLinkInput *input = nullptr;
    const HRESULT hr = as<IDeckLink>(device)->QueryInterface(IID_IDeckLinkInput, reinterpret_cast<void **>(&input));
    *out = input;
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_device_query_output(rdl_handle device, rdl_handle *out)
{
    if (!device || !out) {
        return kInvalidArg;
    }
    IDeckLinkOutput *output = nullptr;
    const HRESULT hr = as<IDeckLink>(device)->QueryInterface(IID_IDeckLinkOutput, reinterpret_cast<void **>(&output));
    *out = output;
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_input_display_mode_iterator(rdl_handle input, rdl_handle *out)
{
    if (!input || !out) {
        return kInvalidArg;
    }
    IDeckLinkDisplayModeIterator *iterator = nullptr;
    const HRESULT hr = as<IDeckLinkInput>(input)->GetDisplayModeIterator(&iterator);
    *out = iterator;
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_output_display_mode_iterator(rdl_handle output, rdl_handle *out)
{
    if (!output || !out) {
        return kInvalidArg;
    }
    IDeckLinkDisplayModeIterator *iterator = nullptr;
    const HRESULT hr = as<IDeckLinkOutput>(output)->GetDisplayModeIterator(&iterator);
    *out = iterator;
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_display_mode_iterator_next(rdl_handle iterator, rdl_handle *mode)
{
    if (!iterator || !mode) {
        return kInvalidArg;
    }
    IDeckLinkDisplayMode *next = nullptr;
    const HRESULT hr = as<IDeckLinkDisplayModeIterator>(iterator)->Next(&next);
    *mode = next;
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_display_mode_info(rdl_handle mode, rdl_display_mode_info_t *info)
{
    if (!mode || !info) {
        return kInvalidArg;
    }
    std::memset(info, 0, sizeof(*info));
    InputCallback::fill_display_mode(as<IDeckLinkDisplayMode>(mode), info);
    return kOk;
}

rdl_hresult rdl_input_does_support(
    rdl_handle input,
    uint32_t connection,
    uint32_t mode,
    uint32_t pixel_format,
    uint32_t conversion,
    uint32_t flags,
    uint32_t *actual_mode,
    int32_t *supported)
{
    if (!input) {
        return kInvalidArg;
    }
    BMDDisplayMode actual = bmdModeUnknown;
#ifdef _WIN32
    BOOL ok = FALSE;
#else
    bool ok = false;
#endif
    const HRESULT hr = as<IDeckLinkInput>(input)->DoesSupportVideoMode(
        static_cast<BMDVideoConnection>(connection),
        static_cast<BMDDisplayMode>(mode),
        static_cast<BMDPixelFormat>(pixel_format),
        static_cast<BMDVideoInputConversionMode>(conversion),
        static_cast<BMDSupportedVideoModeFlags>(flags),
        &actual,
        &ok);
    if (actual_mode) {
        *actual_mode = actual;
    }
    if (supported) {
        *supported = ok ? 1 : 0;
    }
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_input_set_callback(rdl_handle input, const rdl_input_callbacks *callbacks)
{
    if (!input) {
        return kInvalidArg;
    }
    if (!callbacks) {
        return static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->SetCallback(nullptr));
    }
    InputCallback *callback = new (std::nothrow) InputCallback(*callbacks);
    if (!callback) {
        return static_cast<rdl_hresult>(0x80000002);
    }
    const HRESULT hr = as<IDeckLinkInput>(input)->SetCallback(callback);
    callback->Release();
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_input_enable_video(rdl_handle input, uint32_t mode, uint32_t pixel_format, uint32_t flags)
{
    if (!input) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->EnableVideoInput(
        static_cast<BMDDisplayMode>(mode),
        static_cast<BMDPixelFormat>(pixel_format),
        static_cast<BMDVideoInputFlags>(flags)));
}

rdl_hresult rdl_input_disable_video(rdl_handle input)
{
    return input ? static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->DisableVideoInput()) : kInvalidArg;
}

rdl_hresult rdl_input_enable_audio(rdl_handle input, uint32_t sample_rate, uint32_t sample_type, uint32_t channels)
{
    if (!input) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->EnableAudioInput(
        static_cast<BMDAudioSampleRate>(sample_rate),
        static_cast<BMDAudioSampleType>(sample_type),
        channels));
}

rdl_hresult rdl_input_disable_audio(rdl_handle input)
{
    return input ? static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->DisableAudioInput()) : kInvalidArg;
}

rdl_hresult rdl_input_start(rdl_handle input)
{
    return input ? static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->StartStreams()) : kInvalidArg;
}

rdl_hresult rdl_input_stop(rdl_handle input)
{
    return input ? static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->StopStreams()) : kInvalidArg;
}

rdl_hresult rdl_input_pause(rdl_handle input)
{
    return input ? static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->PauseStreams()) : kInvalidArg;
}

rdl_hresult rdl_input_flush(rdl_handle input)
{
    return input ? static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->FlushStreams()) : kInvalidArg;
}

rdl_hresult rdl_input_available_video_frames(rdl_handle input, uint32_t *count)
{
    if (!input || !count) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkInput>(input)->GetAvailableVideoFrameCount(count));
}

rdl_hresult rdl_video_info(rdl_handle frame, int64_t time_scale, rdl_video_info_t *info)
{
    if (!frame || !info) {
        return kInvalidArg;
    }
    auto *video = as<IDeckLinkVideoInputFrame>(frame);
    info->width = static_cast<int32_t>(video->GetWidth());
    info->height = static_cast<int32_t>(video->GetHeight());
    info->row_bytes = static_cast<int32_t>(video->GetRowBytes());
    info->pixel_format = video->GetPixelFormat();
    info->flags = video->GetFlags();
    BMDTimeValue stream_time = 0;
    BMDTimeValue stream_duration = 0;
    video->GetStreamTime(&stream_time, &stream_duration, time_scale);
    info->stream_time = stream_time;
    info->stream_duration = stream_duration;
    BMDTimeValue hw_time = 0;
    BMDTimeValue hw_duration = 0;
    video->GetHardwareReferenceTimestamp(time_scale, &hw_time, &hw_duration);
    info->hardware_time = hw_time;
    info->hardware_duration = hw_duration;
    return kOk;
}

rdl_hresult rdl_video_map_read(rdl_handle frame, const uint8_t **data, size_t *len)
{
    if (!frame || !data || !len) {
        return kInvalidArg;
    }
    IDeckLinkVideoBuffer *buffer = nullptr;
    HRESULT hr = as_unknown(frame)->QueryInterface(IID_IDeckLinkVideoBuffer, reinterpret_cast<void **>(&buffer));
    if (hr != S_OK || !buffer) {
        return hr == S_OK ? kNoInterface : static_cast<rdl_hresult>(hr);
    }
    hr = buffer->StartAccess(bmdBufferAccessRead);
    if (hr != S_OK) {
        buffer->Release();
        return static_cast<rdl_hresult>(hr);
    }
    void *bytes = nullptr;
    hr = buffer->GetBytes(&bytes);
    uint64_t size = 0;
    buffer->GetSize(&size);
    buffer->Release();
    if (hr != S_OK) {
        return static_cast<rdl_hresult>(hr);
    }
    *data = static_cast<const uint8_t *>(bytes);
    *len = static_cast<size_t>(size);
    return kOk;
}

rdl_hresult rdl_video_unmap_read(rdl_handle frame)
{
    if (!frame) {
        return kInvalidArg;
    }
    IDeckLinkVideoBuffer *buffer = nullptr;
    const HRESULT qi = as_unknown(frame)->QueryInterface(IID_IDeckLinkVideoBuffer, reinterpret_cast<void **>(&buffer));
    if (qi != S_OK || !buffer) {
        return qi == S_OK ? kNoInterface : static_cast<rdl_hresult>(qi);
    }
    const HRESULT hr = buffer->EndAccess(bmdBufferAccessRead);
    buffer->Release();
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_audio_info(rdl_handle packet, int64_t time_scale, rdl_audio_info_t *info)
{
    if (!packet || !info) {
        return kInvalidArg;
    }
    auto *audio = as<IDeckLinkAudioInputPacket>(packet);
    info->sample_frame_count = static_cast<int32_t>(audio->GetSampleFrameCount());
    BMDTimeValue packet_time = 0;
    audio->GetPacketTime(&packet_time, time_scale);
    info->packet_time = packet_time;
    return kOk;
}

rdl_hresult rdl_audio_bytes(rdl_handle packet, const uint8_t **data, size_t *len)
{
    if (!packet || !data || !len) {
        return kInvalidArg;
    }
    auto *audio = as<IDeckLinkAudioInputPacket>(packet);
    void *bytes = nullptr;
    const HRESULT hr = audio->GetBytes(&bytes);
    if (hr != S_OK) {
        return static_cast<rdl_hresult>(hr);
    }
    *data = static_cast<const uint8_t *>(bytes);
    *len = static_cast<size_t>(audio->GetSampleFrameCount());
    return kOk;
}

rdl_hresult rdl_output_does_support(
    rdl_handle output,
    uint32_t connection,
    uint32_t mode,
    uint32_t pixel_format,
    uint32_t conversion,
    uint32_t flags,
    uint32_t *actual_mode,
    int32_t *supported)
{
    if (!output) {
        return kInvalidArg;
    }
    BMDDisplayMode actual = bmdModeUnknown;
#ifdef _WIN32
    BOOL ok = FALSE;
#else
    bool ok = false;
#endif
    const HRESULT hr = as<IDeckLinkOutput>(output)->DoesSupportVideoMode(
        static_cast<BMDVideoConnection>(connection),
        static_cast<BMDDisplayMode>(mode),
        static_cast<BMDPixelFormat>(pixel_format),
        static_cast<BMDVideoOutputConversionMode>(conversion),
        static_cast<BMDSupportedVideoModeFlags>(flags),
        &actual,
        &ok);
    if (actual_mode) {
        *actual_mode = actual;
    }
    if (supported) {
        *supported = ok ? 1 : 0;
    }
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_output_set_callbacks(rdl_handle output, const rdl_output_callbacks *callbacks)
{
    if (!output) {
        return kInvalidArg;
    }
    auto *deck = as<IDeckLinkOutput>(output);
    if (!callbacks) {
        deck->SetScheduledFrameCompletionCallback(nullptr);
        deck->SetAudioCallback(nullptr);
        return kOk;
    }
    OutputCallback *callback = new (std::nothrow) OutputCallback(*callbacks);
    if (!callback) {
        return static_cast<rdl_hresult>(0x80000002);
    }
    HRESULT hr = deck->SetScheduledFrameCompletionCallback(callback);
    if (hr == S_OK) {
        hr = deck->SetAudioCallback(callback);
    }
    callback->Release();
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_output_enable_video(rdl_handle output, uint32_t mode, uint32_t flags)
{
    if (!output) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->EnableVideoOutput(
        static_cast<BMDDisplayMode>(mode), static_cast<BMDVideoOutputFlags>(flags)));
}

rdl_hresult rdl_output_disable_video(rdl_handle output)
{
    return output ? static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->DisableVideoOutput()) : kInvalidArg;
}

rdl_hresult rdl_output_enable_audio(
    rdl_handle output,
    uint32_t sample_rate,
    uint32_t sample_type,
    uint32_t channels,
    uint32_t stream_type)
{
    if (!output) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->EnableAudioOutput(
        static_cast<BMDAudioSampleRate>(sample_rate),
        static_cast<BMDAudioSampleType>(sample_type),
        channels,
        static_cast<BMDAudioOutputStreamType>(stream_type)));
}

rdl_hresult rdl_output_disable_audio(rdl_handle output)
{
    return output ? static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->DisableAudioOutput()) : kInvalidArg;
}

rdl_hresult rdl_output_row_bytes(rdl_handle output, uint32_t pixel_format, int32_t width, int32_t *row_bytes)
{
    if (!output || !row_bytes) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->RowBytesForPixelFormat(
        static_cast<BMDPixelFormat>(pixel_format), width, row_bytes));
}

rdl_hresult rdl_output_create_frame(
    rdl_handle output,
    int32_t width,
    int32_t height,
    int32_t row_bytes,
    uint32_t pixel_format,
    uint32_t flags,
    const uint8_t *data,
    size_t data_len,
    rdl_handle *frame)
{
    if (!output || !frame) {
        return kInvalidArg;
    }
    IDeckLinkMutableVideoFrame *created = nullptr;
    HRESULT hr = as<IDeckLinkOutput>(output)->CreateVideoFrame(
        width,
        height,
        row_bytes,
        static_cast<BMDPixelFormat>(pixel_format),
        static_cast<BMDFrameFlags>(flags),
        &created);
    if (hr != S_OK || !created) {
        *frame = nullptr;
        return static_cast<rdl_hresult>(hr);
    }
    if (data && data_len > 0) {
        IDeckLinkVideoBuffer *buffer = nullptr;
        hr = created->QueryInterface(IID_IDeckLinkVideoBuffer, reinterpret_cast<void **>(&buffer));
        if (hr == S_OK && buffer) {
            if (buffer->StartAccess(bmdBufferAccessWrite) == S_OK) {
                void *bytes = nullptr;
                if (buffer->GetBytes(&bytes) == S_OK && bytes) {
                    uint64_t size = 0;
                    buffer->GetSize(&size);
                    const size_t copy = data_len < static_cast<size_t>(size) ? data_len : static_cast<size_t>(size);
                    std::memcpy(bytes, data, copy);
                }
                buffer->EndAccess(bmdBufferAccessWrite);
            }
            buffer->Release();
        }
    }
    *frame = created;
    return kOk;
}

rdl_hresult rdl_output_schedule_video(
    rdl_handle output,
    rdl_handle frame,
    int64_t display_time,
    int64_t display_duration,
    int64_t time_scale)
{
    if (!output || !frame) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->ScheduleVideoFrame(
        as<IDeckLinkVideoFrame>(frame), display_time, display_duration, time_scale));
}

rdl_hresult rdl_output_schedule_audio(
    rdl_handle output,
    const uint8_t *data,
    uint32_t sample_frame_count,
    int64_t stream_time,
    int64_t time_scale,
    uint32_t *written)
{
    if (!output || !data) {
        return kInvalidArg;
    }
    uint32_t local = 0;
    const HRESULT hr = as<IDeckLinkOutput>(output)->ScheduleAudioSamples(
        const_cast<uint8_t *>(data), sample_frame_count, stream_time, time_scale, &local);
    if (written) {
        *written = local;
    }
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_output_begin_audio_preroll(rdl_handle output)
{
    return output ? static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->BeginAudioPreroll()) : kInvalidArg;
}

rdl_hresult rdl_output_end_audio_preroll(rdl_handle output)
{
    return output ? static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->EndAudioPreroll()) : kInvalidArg;
}

rdl_hresult rdl_output_start(rdl_handle output, int64_t start_time, int64_t time_scale, double speed)
{
    return output
        ? static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->StartScheduledPlayback(start_time, time_scale, speed))
        : kInvalidArg;
}

rdl_hresult rdl_output_stop(rdl_handle output, int64_t stop_time, int64_t time_scale, int64_t *actual)
{
    if (!output) {
        return kInvalidArg;
    }
    BMDTimeValue local = 0;
    const HRESULT hr = as<IDeckLinkOutput>(output)->StopScheduledPlayback(stop_time, &local, time_scale);
    if (actual) {
        *actual = local;
    }
    return static_cast<rdl_hresult>(hr);
}

rdl_hresult rdl_output_buffered_video(rdl_handle output, uint32_t *count)
{
    if (!output || !count) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->GetBufferedVideoFrameCount(count));
}

rdl_hresult rdl_output_buffered_audio(rdl_handle output, uint32_t *count)
{
    if (!output || !count) {
        return kInvalidArg;
    }
    return static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->GetBufferedAudioSampleFrameCount(count));
}

rdl_hresult rdl_output_flush_audio(rdl_handle output)
{
    return output ? static_cast<rdl_hresult>(as<IDeckLinkOutput>(output)->FlushBufferedAudioSamples()) : kInvalidArg;
}

}

#endif
