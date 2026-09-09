use decklink::{
    AudioConfig, CaptureEvent, DeckLinkContext, DisplayMode, FrameCompletion, MockAudio, MockCaptureEvent, MockVideo,
    MockWorld, OverflowPolicy, PixelFormat, PlayoutEvent, SampleType, ScheduledAudioPacket, ScheduledVideoFrame, Time,
};
use futures_util::{FutureExt, StreamExt};

fn world_with(events: Vec<MockCaptureEvent>) -> MockWorld {
    let mut world = MockWorld::demo();
    world.capture_events = events;
    world
}

#[test]
fn enumerates_mock_device() {
    let ctx = DeckLinkContext::mock(MockWorld::demo());
    let devices = ctx.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].info.display_name, "Mock DeckLink");
    assert!(!ctx.api_version().unwrap().is_empty());
}

#[test]
fn capture_delivers_sample_and_format_change() {
    let mode = DisplayMode::hd1080p30();
    let world = world_with(vec![
        MockCaptureEvent::Sample {
            video: Some(MockVideo {
                width: 8,
                height: 2,
                row_bytes: 16,
                pixel_format: PixelFormat::YUV_8BIT,
                flags: 0,
                bytes: vec![1; 32],
                stream_time: Time::new(0, 30_000).ok(),
            }),
            audio: Some(MockAudio {
                config: AudioConfig::default(),
                bytes: vec![0; AudioConfig::default().byte_len(4)],
                packet_time: Time::new(0, 30_000).ok(),
            }),
        },
        MockCaptureEvent::FormatChanged(decklink::DetectedFormat {
            events: 1,
            mode: mode.clone(),
            flags: 0,
        }),
    ]);
    let ctx = DeckLinkContext::mock(world);
    let device = ctx.first_device().unwrap();
    pollster::block_on(async {
        let mut capture = device
            .capture()
            .video(mode, PixelFormat::YUV_8BIT)
            .audio(AudioConfig::default())
            .start()
            .await
            .unwrap();
        let first = capture.next().await.unwrap().unwrap();
        match first {
            CaptureEvent::Sample(sample) => {
                let frame = sample.video.unwrap();
                assert_eq!(frame.width(), 8);
                assert!(frame.has_input_source());
                assert_eq!(frame.map_read().unwrap().as_bytes().len(), 32);
                assert_eq!(sample.audio.unwrap().sample_frames(), 4);
            }
            other => panic!("unexpected {other:?}"),
        }
        let second = capture.next().await.unwrap().unwrap();
        assert!(matches!(second, CaptureEvent::FormatChanged(_)));
        capture.shutdown().await.unwrap();
    });
}

#[test]
fn capture_null_video_keeps_audio() {
    let world = world_with(vec![MockCaptureEvent::Sample {
        video: None,
        audio: Some(MockAudio {
            config: AudioConfig::default(),
            bytes: vec![0; AudioConfig::default().byte_len(2)],
            packet_time: None,
        }),
    }]);
    let ctx = DeckLinkContext::mock(world);
    let device = ctx.first_device().unwrap();
    pollster::block_on(async {
        let mut capture = device.capture().start().await.unwrap();
        match capture.next().await.unwrap().unwrap() {
            CaptureEvent::Sample(sample) => {
                assert!(sample.video.is_none());
                assert!(sample.audio.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }
        capture.shutdown().await.unwrap();
    });
}

#[test]
fn overflow_error_and_stop_closes_stream() {
    let mut events = Vec::new();
    for i in 0..8 {
        events.push(MockCaptureEvent::Sample {
            video: Some(MockVideo {
                width: 2,
                height: 1,
                row_bytes: 4,
                pixel_format: PixelFormat::YUV_8BIT,
                flags: 0,
                bytes: vec![i as u8; 4],
                stream_time: None,
            }),
            audio: None,
        });
    }
    let mut world = world_with(events);
    world.capture_interval = None;
    let ctx = DeckLinkContext::mock(world);
    let device = ctx.first_device().unwrap();
    pollster::block_on(async {
        let mut capture = device
            .capture()
            .queue_capacity(1)
            .overflow(OverflowPolicy::ErrorAndStop)
            .start()
            .await
            .unwrap();
        let mut saw_overflow = false;
        while let Some(item) = capture.next().await {
            if matches!(item, Ok(CaptureEvent::Overflow(_))) {
                saw_overflow = true;
                break;
            }
        }
        assert!(saw_overflow);
        capture.shutdown().await.unwrap();
    });
}

#[test]
fn playout_reports_completion() {
    let ctx = DeckLinkContext::mock(MockWorld::demo());
    let device = ctx.first_device().unwrap();
    let mode = DisplayMode::hd1080p30();
    pollster::block_on(async {
        let mut playout = device
            .playout()
            .video(mode.clone(), PixelFormat::YUV_8BIT)
            .start()
            .await
            .unwrap();
        let frame = ScheduledVideoFrame {
            width: 8,
            height: 2,
            row_bytes: 16,
            pixel_format: PixelFormat::YUV_8BIT,
            flags: 0,
            display_time: Time::new(0, 30_000).unwrap(),
            display_duration: Time::new(1001, 30_000).unwrap(),
            bytes: vec![0; 32],
        };
        let token = playout.schedule_video(frame).await.unwrap();
        match playout.next().await.unwrap().unwrap() {
            PlayoutEvent::FrameCompleted {
                token: completed,
                result,
            } => {
                assert_eq!(completed, token);
                assert_eq!(result, FrameCompletion::Completed);
            }
            other => panic!("unexpected {other:?}"),
        }
        playout.shutdown().await.unwrap();
    });
}

#[test]
fn partial_audio_write_is_visible() {
    let ctx = DeckLinkContext::mock(MockWorld::demo());
    let device = ctx.first_device().unwrap();
    pollster::block_on(async {
        let playout = device
            .playout()
            .audio(AudioConfig {
                sample_rate: 48_000,
                sample_type: SampleType::Integer16,
                channels: 2,
            })
            .start()
            .await
            .unwrap();
        let written = playout
            .schedule_audio(ScheduledAudioPacket {
                stream_time: Time::new(0, 48_000).unwrap(),
                bytes: vec![0; 64],
            })
            .await
            .unwrap();
        assert!(written > 0);
        // Drop without waiting; shutdown is best-effort from Drop.
        let _ = playout.events().next().now_or_never();
    });
}

#[tokio::test]
async fn concurrent_shutdown_does_not_panic() {
    let ctx = DeckLinkContext::mock(MockWorld::demo());
    let device = ctx.first_device().unwrap();
    let mut capture = device.capture().start().await.unwrap();
    let mut playout = device.playout().start().await.unwrap();
    let _ = tokio::join!(capture.shutdown(), playout.shutdown());
}
