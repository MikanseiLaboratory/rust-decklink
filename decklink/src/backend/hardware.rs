#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::undocumented_unsafe_blocks)]

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use crate::audio::{AudioConfig, CapturedAudioPacket, ScheduledAudioPacket};
use crate::backend::{Backend, DeviceSnapshot, InputConfig, InputSink, OutputConfig, OutputSink};
use crate::device::{DeviceId, DeviceInfo};
use crate::error::{Error, ErrorKind, Hresult, Result};
use crate::frame::{CapturedVideoFrame, FrameCompletion, ScheduledVideoFrame};
use crate::gpu::GpuRegistry;
use crate::mode::{DetectedFormat, DisplayMode, DisplayModeId, FieldDominance};
use crate::time::Time;

struct InputBridge {
    sink: Box<dyn InputSink>,
    audio: Option<AudioConfig>,
    time_scale: i64,
    gpu: Option<std::sync::Arc<GpuRegistry>>,
}

struct InFlightFrame {
    handle: decklink_sys::Handle,
    token: u64,
    _keep: ScheduledVideoFrame,
}

struct OutputBridge {
    sink: Box<dyn OutputSink>,
    in_flight: Mutex<Vec<InFlightFrame>>,
}

pub struct HardwareBackend {
    initialized: bool,
    device: decklink_sys::Handle,
    input: decklink_sys::Handle,
    output: decklink_sys::Handle,
    input_bridge: Option<*mut InputBridge>,
    output_bridge: Option<*mut OutputBridge>,
    output_audio: Option<AudioConfig>,
    output_time_scale: i64,
    output_started: bool,
    preroll_queued: u32,
}

impl HardwareBackend {
    pub fn new() -> Result<Self> {
        let hr = unsafe { decklink_sys::rdl_initialize() };
        if hr == decklink_sys::E_NOTIMPL {
            return Err(Error::new(
                ErrorKind::DriverNotFound,
                "initialize",
                "hardware shim was not compiled; enable the `hardware` feature and set DECKLINK_SDK_DIR",
            ));
        }
        Error::check("initialize", hr)?;
        if unsafe { decklink_sys::rdl_hardware_available() } == 0 {
            return Err(Error::new(
                ErrorKind::AbiMismatch,
                "initialize",
                "DeckLink shim built without a usable SDK",
            ));
        }
        Ok(Self {
            initialized: true,
            device: std::ptr::null_mut(),
            input: std::ptr::null_mut(),
            output: std::ptr::null_mut(),
            input_bridge: None,
            output_bridge: None,
            output_audio: None,
            output_time_scale: 30_000,
            output_started: false,
            preroll_queued: 0,
        })
    }
}

impl Drop for HardwareBackend {
    fn drop(&mut self) {
        let _ = self.shutdown();
        if self.initialized {
            unsafe { decklink_sys::rdl_uninitialize() };
        }
    }
}

impl Backend for HardwareBackend {
    fn enumerate(&mut self) -> Result<Vec<DeviceSnapshot>> {
        let mut iterator = std::ptr::null_mut();
        Error::check("iterator_create", unsafe {
            decklink_sys::rdl_iterator_create(&mut iterator)
        })?;
        let mut devices = Vec::new();
        loop {
            let mut device = std::ptr::null_mut();
            let hr = unsafe { decklink_sys::rdl_iterator_next(iterator, &mut device) };
            if hr == decklink_sys::S_FALSE || device.is_null() {
                break;
            }
            Error::check("iterator_next", hr)?;
            devices.push(DeviceSnapshot {
                info: unsafe { device_info(device)? },
            });
            unsafe { decklink_sys::rdl_release(device) };
        }
        unsafe { decklink_sys::rdl_release(iterator) };
        Ok(devices)
    }

    fn open(&mut self, id: &DeviceId) -> Result<()> {
        let devices = self.enumerate()?;
        let mut iterator = std::ptr::null_mut();
        Error::check("iterator_create", unsafe {
            decklink_sys::rdl_iterator_create(&mut iterator)
        })?;
        let mut found = std::ptr::null_mut();
        loop {
            let mut device = std::ptr::null_mut();
            let hr = unsafe { decklink_sys::rdl_iterator_next(iterator, &mut device) };
            if hr == decklink_sys::S_FALSE || device.is_null() {
                break;
            }
            if let Ok(info) = unsafe { device_info(device) } {
                if &info.id == id {
                    found = device;
                    break;
                }
            }
            unsafe { decklink_sys::rdl_release(device) };
        }
        unsafe { decklink_sys::rdl_release(iterator) };
        if found.is_null() {
            let _ = devices;
            return Err(Error::new(ErrorKind::DriverNotFound, "open", "device disappeared"));
        }
        self.device = found;
        Ok(())
    }

    fn display_modes(&mut self, capture: bool) -> Result<Vec<DisplayMode>> {
        self.ensure_io(capture)?;
        let handle = if capture { self.input } else { self.output };
        let mut iterator = std::ptr::null_mut();
        let hr = if capture {
            unsafe { decklink_sys::rdl_input_display_mode_iterator(handle, &mut iterator) }
        } else {
            unsafe { decklink_sys::rdl_output_display_mode_iterator(handle, &mut iterator) }
        };
        Error::check("display_mode_iterator", hr)?;
        let mut modes = Vec::new();
        loop {
            let mut mode = std::ptr::null_mut();
            let hr = unsafe { decklink_sys::rdl_display_mode_iterator_next(iterator, &mut mode) };
            if hr == decklink_sys::S_FALSE || mode.is_null() {
                break;
            }
            Error::check("display_mode_next", hr)?;
            modes.push(unsafe { display_mode_from_handle(mode)? });
            unsafe { decklink_sys::rdl_release(mode) };
        }
        unsafe { decklink_sys::rdl_release(iterator) };
        Ok(modes)
    }

    fn enable_input(&mut self, config: &InputConfig, sink: Box<dyn InputSink>) -> Result<()> {
        self.ensure_io(true)?;
        let gpu = config
            .gpu
            .as_ref()
            .map(|factory| GpuRegistry::new(std::sync::Arc::clone(factory)));
        let bridge = Box::into_raw(Box::new(InputBridge {
            sink,
            audio: config.audio,
            time_scale: config.time_scale,
            gpu,
        }));
        self.input_bridge = Some(bridge);
        let callbacks = decklink_sys::InputCallbacks {
            ctx: bridge.cast(),
            frame: Some(on_input_frame),
            format: Some(on_input_format),
        };
        Error::check("input_set_callback", unsafe {
            decklink_sys::rdl_input_set_callback(self.input, &callbacks)
        })?;
        if let Some(registry) = unsafe { (*bridge).gpu.as_ref() } {
            Error::check("input_enable_video_allocator", unsafe {
                decklink_sys::rdl_input_enable_video_with_allocator(
                    self.input,
                    config.mode.id.0,
                    config.pixel_format.0,
                    config.flags.0,
                    std::sync::Arc::as_ptr(registry) as *mut _,
                    crate::gpu::alloc_video_buffer,
                    crate::gpu::free_video_buffer,
                )
            })?;
        } else {
            Error::check("input_enable_video", unsafe {
                decklink_sys::rdl_input_enable_video(
                    self.input,
                    config.mode.id.0,
                    config.pixel_format.0,
                    config.flags.0,
                )
            })?;
        }
        if let Some(audio) = config.audio {
            Error::check("input_enable_audio", unsafe {
                decklink_sys::rdl_input_enable_audio(
                    self.input,
                    audio.sample_rate,
                    audio.sample_type.raw(),
                    audio.channels,
                )
            })?;
        }
        Ok(())
    }

    fn start_input(&mut self) -> Result<()> {
        Error::check("input_start", unsafe { decklink_sys::rdl_input_start(self.input) })
    }

    fn stop_input(&mut self) -> Result<()> {
        if !self.input.is_null() {
            let _ = unsafe { decklink_sys::rdl_input_stop(self.input) };
            let _ = unsafe { decklink_sys::rdl_input_flush(self.input) };
            let _ = unsafe { decklink_sys::rdl_input_disable_audio(self.input) };
            let _ = unsafe { decklink_sys::rdl_input_disable_video(self.input) };
            let _ = unsafe { decklink_sys::rdl_input_set_callback(self.input, std::ptr::null()) };
        }
        if let Some(bridge) = self.input_bridge.take() {
            unsafe { drop(Box::from_raw(bridge)) };
        }
        Ok(())
    }

    fn enable_output(&mut self, config: &OutputConfig, sink: Box<dyn OutputSink>) -> Result<()> {
        self.ensure_io(false)?;
        let _ = config.pixel_format;
        self.output_audio = config.audio;
        self.output_time_scale = config.mode.frame_duration.scale.get();
        self.output_started = false;
        self.preroll_queued = 0;
        let bridge = Box::into_raw(Box::new(OutputBridge {
            sink,
            in_flight: Mutex::new(Vec::new()),
        }));
        self.output_bridge = Some(bridge);
        let callbacks = decklink_sys::OutputCallbacks {
            ctx: bridge.cast(),
            completed: Some(on_output_completed),
            stopped: Some(on_output_stopped),
            audio_render: Some(on_audio_render),
        };
        Error::check("output_set_callbacks", unsafe {
            decklink_sys::rdl_output_set_callbacks(self.output, &callbacks)
        })?;
        Error::check("output_enable_video", unsafe {
            decklink_sys::rdl_output_enable_video(self.output, config.mode.id.0, config.flags.0)
        })?;
        if let Some(audio) = config.audio {
            Error::check("output_enable_audio", unsafe {
                decklink_sys::rdl_output_enable_audio(
                    self.output,
                    audio.sample_rate,
                    audio.sample_type.raw(),
                    audio.channels,
                    decklink_sys::AUDIO_OUTPUT_STREAM_TIMESTAMPED,
                )
            })?;
        }
        Ok(())
    }

    fn schedule_video(&mut self, token: u64, frame: ScheduledVideoFrame) -> Result<()> {
        frame.validate()?;
        let (cpu, size) = if let Some(gpu) = &frame.gpu {
            (gpu.cpu_ptr as *mut c_void, gpu.size as u64)
        } else {
            (frame.bytes.as_ptr() as *mut c_void, frame.bytes.len() as u64)
        };
        let mut handle = std::ptr::null_mut();
        Error::check("create_frame", unsafe {
            decklink_sys::rdl_output_create_frame_from_external(
                self.output,
                frame.width,
                frame.height,
                frame.row_bytes,
                frame.pixel_format.0,
                frame.flags,
                cpu,
                size,
                &mut handle,
            )
        })?;
        let hr = unsafe {
            decklink_sys::rdl_output_schedule_video(
                self.output,
                handle,
                frame.display_time.value,
                frame.display_duration.value,
                frame.display_time.scale.get(),
            )
        };
        if decklink_sys::failed(hr) {
            unsafe { decklink_sys::rdl_release(handle) };
            return Err(Error::sdk("schedule_video", Hresult(hr)));
        }
        if let Some(bridge) = self.output_bridge {
            unsafe {
                (*bridge).in_flight.lock().expect("in_flight").push(InFlightFrame {
                    handle,
                    token,
                    _keep: frame,
                });
            }
        }
        self.preroll_queued = self.preroll_queued.saturating_add(1);
        self.kick_playback_if_ready()
    }

    fn schedule_audio(&mut self, packet: &ScheduledAudioPacket) -> Result<u32> {
        if !self.output_started {
            return Ok(0);
        }
        let bytes_per_frame = self.output_audio.unwrap_or_default().bytes_per_frame().max(1);
        let mut written = 0u32;
        Error::check("schedule_audio", unsafe {
            decklink_sys::rdl_output_schedule_audio(
                self.output,
                packet.bytes.as_ptr(),
                (packet.bytes.len() / bytes_per_frame) as u32,
                packet.stream_time.value,
                packet.stream_time.scale.get(),
                &mut written,
            )
        })?;
        Ok(written)
    }

    fn start_output(&mut self) -> Result<()> {
        Error::check("start_output", unsafe {
            decklink_sys::rdl_output_start(self.output, 0, self.output_time_scale, 1.0)
        })?;
        self.output_started = true;
        Ok(())
    }

    fn stop_output(&mut self) -> Result<()> {
        self.output_started = false;
        self.preroll_queued = 0;
        if !self.output.is_null() {
            let mut actual = 0i64;
            let _ = unsafe { decklink_sys::rdl_output_stop(self.output, 0, self.output_time_scale, &mut actual) };
            let _ = unsafe { decklink_sys::rdl_output_flush_audio(self.output) };
            let _ = unsafe { decklink_sys::rdl_output_disable_audio(self.output) };
            let _ = unsafe { decklink_sys::rdl_output_disable_video(self.output) };
            let _ = unsafe { decklink_sys::rdl_output_set_callbacks(self.output, std::ptr::null()) };
        }
        if let Some(bridge) = self.output_bridge.take() {
            unsafe {
                for pending in (*bridge).in_flight.lock().expect("in_flight").drain(..) {
                    decklink_sys::rdl_release(pending.handle);
                }
                drop(Box::from_raw(bridge));
            }
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        let _ = self.stop_input();
        let _ = self.stop_output();
        if !self.input.is_null() {
            unsafe { decklink_sys::rdl_release(self.input) };
            self.input = std::ptr::null_mut();
        }
        if !self.output.is_null() {
            unsafe { decklink_sys::rdl_release(self.output) };
            self.output = std::ptr::null_mut();
        }
        if !self.device.is_null() {
            unsafe { decklink_sys::rdl_release(self.device) };
            self.device = std::ptr::null_mut();
        }
        Ok(())
    }
}

impl HardwareBackend {
    fn kick_playback_if_ready(&mut self) -> Result<()> {
        const PREROLL_FRAMES: u32 = 3;
        if self.output_started || self.preroll_queued < PREROLL_FRAMES {
            return Ok(());
        }
        self.start_output()
    }

    fn ensure_io(&mut self, capture: bool) -> Result<()> {
        if self.device.is_null() {
            return Err(Error::new(ErrorKind::InvalidState, "io", "device is not open"));
        }
        if capture && self.input.is_null() {
            Error::check("query_input", unsafe {
                decklink_sys::rdl_device_query_input(self.device, &mut self.input)
            })?;
        }
        if !capture && self.output.is_null() {
            Error::check("query_output", unsafe {
                decklink_sys::rdl_device_query_output(self.device, &mut self.output)
            })?;
        }
        Ok(())
    }
}

unsafe fn device_info(device: decklink_sys::Handle) -> Result<DeviceInfo> {
    let mut model = [0u8; 128];
    let mut display = [0u8; 128];
    Error::check(
        "model_name",
        decklink_sys::rdl_device_model_name(device, model.as_mut_ptr(), model.len()),
    )?;
    Error::check(
        "display_name",
        decklink_sys::rdl_device_display_name(device, display.as_mut_ptr(), display.len()),
    )?;
    let model_name = decklink_sys::cstr_from_buf(&model);
    let display_name = decklink_sys::cstr_from_buf(&display);
    let mut persistent = 0i64;
    let persistent_id = if decklink_sys::succeeded(decklink_sys::rdl_device_profile_int(
        device,
        decklink_sys::ATTR_PERSISTENT_ID,
        &mut persistent,
    )) {
        Some(persistent as u64)
    } else {
        None
    };
    let mut topological = 0i64;
    let topological_id = if decklink_sys::succeeded(decklink_sys::rdl_device_profile_int(
        device,
        decklink_sys::ATTR_TOPOLOGICAL_ID,
        &mut topological,
    )) {
        Some(topological as u32)
    } else {
        None
    };
    let mut io = 0i64;
    let _ = decklink_sys::rdl_device_profile_int(device, decklink_sys::ATTR_VIDEO_IO_SUPPORT, &mut io);
    let mut format_detection = 0i32;
    let _ = decklink_sys::rdl_device_profile_flag(
        device,
        decklink_sys::ATTR_SUPPORTS_INPUT_FORMAT_DETECTION,
        &mut format_detection,
    );
    let mut channels = 0i64;
    let maximum_audio_channels = if decklink_sys::succeeded(decklink_sys::rdl_device_profile_int(
        device,
        decklink_sys::ATTR_MAXIMUM_AUDIO_CHANNELS,
        &mut channels,
    )) {
        Some(channels as u32)
    } else {
        None
    };
    let mut preroll = 0i64;
    let minimum_preroll_frames = if decklink_sys::succeeded(decklink_sys::rdl_device_profile_int(
        device,
        decklink_sys::ATTR_MINIMUM_PREROLL_FRAMES,
        &mut preroll,
    )) {
        Some(preroll as u32)
    } else {
        None
    };
    Ok(DeviceInfo {
        id: DeviceId {
            persistent_id,
            topological_id,
            display_name: display_name.clone(),
        },
        model_name,
        display_name,
        supports_capture: io & decklink_sys::IO_SUPPORT_CAPTURE != 0 || io == 0,
        supports_playback: io & decklink_sys::IO_SUPPORT_PLAYBACK != 0 || io == 0,
        supports_format_detection: format_detection != 0,
        maximum_audio_channels,
        minimum_preroll_frames,
    })
}

unsafe fn display_mode_from_handle(mode: decklink_sys::Handle) -> Result<DisplayMode> {
    let mut info = decklink_sys::DisplayModeInfo::default();
    Error::check(
        "display_mode_info",
        decklink_sys::rdl_display_mode_info(mode, &mut info),
    )?;
    Ok(DisplayMode {
        id: DisplayModeId(info.mode),
        name: decklink_sys::cstr_from_buf(&info.name),
        width: info.width,
        height: info.height,
        frame_duration: Time::new(info.frame_duration, info.time_scale)?,
        field_dominance: FieldDominance(info.field_dominance),
        flags: info.flags,
    })
}

unsafe extern "C" fn on_input_frame(ctx: *mut c_void, video: decklink_sys::Handle, audio: decklink_sys::Handle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(bridge) = ctx.cast::<InputBridge>().as_ref() else {
            release_pair(video, audio);
            return;
        };
        let video = if video.is_null() {
            None
        } else {
            match CapturedVideoFrame::from_hardware(video, bridge.time_scale, bridge.gpu.as_deref()) {
                Ok(frame) => Some(frame),
                Err(_) => {
                    decklink_sys::rdl_release(video);
                    None
                }
            }
        };
        let audio = if audio.is_null() {
            None
        } else if let Some(config) = bridge.audio {
            match CapturedAudioPacket::from_hardware(audio, config, bridge.time_scale) {
                Ok(packet) => Some(packet),
                Err(_) => {
                    decklink_sys::rdl_release(audio);
                    None
                }
            }
        } else {
            decklink_sys::rdl_release(audio);
            None
        };
        bridge.sink.frame(video, audio);
    }));
}

unsafe extern "C" fn on_input_format(
    ctx: *mut c_void,
    events: u32,
    mode: u32,
    width: i32,
    height: i32,
    duration: i64,
    scale: i64,
    flags: u32,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(bridge) = ctx.cast::<InputBridge>().as_ref() else {
            return;
        };
        if let Ok(frame_duration) = Time::new(duration, scale) {
            bridge.sink.format_changed(DetectedFormat {
                events,
                mode: DisplayMode {
                    id: DisplayModeId(mode),
                    name: format!("detected-{mode:08x}"),
                    width,
                    height,
                    frame_duration,
                    field_dominance: FieldDominance::UNKNOWN,
                    flags: 0,
                },
                flags,
            });
        }
    }));
}

unsafe extern "C" fn on_output_completed(ctx: *mut c_void, frame: decklink_sys::Handle, result: u32) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(bridge) = ctx.cast::<OutputBridge>().as_ref() else {
            if !frame.is_null() {
                decklink_sys::rdl_release(frame);
            }
            return;
        };
        let (token, owned) = {
            let mut in_flight = bridge.in_flight.lock().expect("in_flight");
            match in_flight.iter().position(|pending| pending.handle == frame) {
                Some(index) => (in_flight.remove(index).token, true),
                None => (frame as u64, false),
            }
        };
        if !frame.is_null() {
            if owned {
                // Drop the CreateVideoFrame / CreateVideoFrameWithBuffer ref kept in `in_flight`.
                decklink_sys::rdl_release(frame);
            }
            // Drop the extra AddRef from ScheduledFrameCompleted.
            decklink_sys::rdl_release(frame);
        }
        bridge.sink.completed(token, FrameCompletion::from_raw(result));
    }));
}

unsafe extern "C" fn on_output_stopped(ctx: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(bridge) = ctx.cast::<OutputBridge>().as_ref() {
            bridge.sink.stopped();
        }
    }));
}

unsafe extern "C" fn on_audio_render(ctx: *mut c_void, preroll: i32) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(bridge) = ctx.cast::<OutputBridge>().as_ref() {
            bridge.sink.render_audio(preroll != 0);
        }
    }));
}

fn release_pair(video: decklink_sys::Handle, audio: decklink_sys::Handle) {
    unsafe {
        if !video.is_null() {
            decklink_sys::rdl_release(video);
        }
        if !audio.is_null() {
            decklink_sys::rdl_release(audio);
        }
    }
}
