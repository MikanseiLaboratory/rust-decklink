use std::thread::{self, JoinHandle};

use futures_channel::oneshot;

use crate::audio::ScheduledAudioPacket;
use crate::backend::{Backend, InputConfig, InputSink, OutputConfig, OutputSink};
use crate::device::DeviceId;
use crate::error::{Error, ErrorKind, Result};
use crate::frame::ScheduledVideoFrame;
use crate::mode::DisplayMode;

enum Command {
    Enumerate(oneshot::Sender<Result<Vec<crate::backend::DeviceSnapshot>>>),
    Open(DeviceId, oneshot::Sender<Result<()>>),
    DisplayModes(bool, oneshot::Sender<Result<Vec<DisplayMode>>>),
    EnableInput(InputConfig, Box<dyn InputSink>, oneshot::Sender<Result<()>>),
    StartInput(oneshot::Sender<Result<()>>),
    StopInput(oneshot::Sender<Result<()>>),
    EnableOutput(OutputConfig, Box<dyn OutputSink>, oneshot::Sender<Result<()>>),
    ScheduleVideo(u64, ScheduledVideoFrame, oneshot::Sender<Result<()>>),
    ScheduleAudio(ScheduledAudioPacket, oneshot::Sender<Result<u32>>),
    StopOutput(oneshot::Sender<Result<()>>),
    Shutdown(oneshot::Sender<Result<()>>),
}

/// Dedicated OS thread that owns DeckLink COM objects.
#[derive(Clone, Debug)]
pub struct ActorHandle {
    tx: std::sync::mpsc::Sender<Command>,
}

pub struct ActorJoin {
    handle: Option<JoinHandle<()>>,
}

impl ActorHandle {
    pub fn spawn<F>(factory: F) -> Result<(Self, ActorJoin)>
    where
        F: FnOnce() -> Result<Box<dyn Backend>> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = thread::Builder::new()
            .name("decklink-actor".into())
            .spawn(move || {
                let mut backend = match factory() {
                    Ok(backend) => {
                        let _ = ready_tx.send(Ok(()));
                        backend
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                while let Ok(command) = rx.recv() {
                    if !dispatch(backend.as_mut(), command) {
                        break;
                    }
                }
                let _ = backend.shutdown();
            })
            .map_err(|err| Error::new(ErrorKind::Sdk, "actor", err.to_string()))?;
        ready_rx
            .recv()
            .map_err(|_| Error::new(ErrorKind::Disconnected, "actor", "actor failed during startup"))??;
        Ok((Self { tx }, ActorJoin { handle: Some(thread) }))
    }

    #[allow(dead_code)]
    pub async fn enumerate(&self) -> Result<Vec<crate::backend::DeviceSnapshot>> {
        self.request(Command::Enumerate).await
    }

    pub fn enumerate_blocking(&self) -> Result<Vec<crate::backend::DeviceSnapshot>> {
        self.request_blocking(Command::Enumerate)
    }

    pub async fn open(&self, id: DeviceId) -> Result<()> {
        self.request(|tx| Command::Open(id, tx)).await
    }

    pub fn open_blocking(&self, id: DeviceId) -> Result<()> {
        self.request_blocking(|tx| Command::Open(id, tx))
    }

    #[allow(dead_code)]
    pub async fn display_modes(&self, capture: bool) -> Result<Vec<DisplayMode>> {
        self.request(|tx| Command::DisplayModes(capture, tx)).await
    }

    pub fn display_modes_blocking(&self, capture: bool) -> Result<Vec<DisplayMode>> {
        self.request_blocking(|tx| Command::DisplayModes(capture, tx))
    }

    pub async fn enable_input(&self, config: InputConfig, sink: Box<dyn InputSink>) -> Result<()> {
        self.request(|tx| Command::EnableInput(config, sink, tx)).await
    }

    pub async fn start_input(&self) -> Result<()> {
        self.request(Command::StartInput).await
    }

    pub async fn stop_input(&self) -> Result<()> {
        self.request(Command::StopInput).await
    }

    pub fn stop_input_now(&self) {
        let (tx, _rx) = oneshot::channel();
        let _ = self.tx.send(Command::StopInput(tx));
    }

    pub async fn enable_output(&self, config: OutputConfig, sink: Box<dyn OutputSink>) -> Result<()> {
        self.request(|tx| Command::EnableOutput(config, sink, tx)).await
    }

    pub async fn schedule_video(&self, token: u64, frame: ScheduledVideoFrame) -> Result<()> {
        self.request(|tx| Command::ScheduleVideo(token, frame, tx)).await
    }

    pub fn begin_schedule_video(
        &self,
        token: u64,
        frame: ScheduledVideoFrame,
    ) -> Result<oneshot::Receiver<Result<()>>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::ScheduleVideo(token, frame, tx))
            .map_err(|_| Error::new(ErrorKind::Disconnected, "actor", "actor thread is gone"))?;
        Ok(rx)
    }

    pub async fn schedule_audio(&self, packet: ScheduledAudioPacket) -> Result<u32> {
        self.request(|tx| Command::ScheduleAudio(packet, tx)).await
    }

    pub fn begin_schedule_audio(&self, packet: ScheduledAudioPacket) -> Result<oneshot::Receiver<Result<u32>>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::ScheduleAudio(packet, tx))
            .map_err(|_| Error::new(ErrorKind::Disconnected, "actor", "actor thread is gone"))?;
        Ok(rx)
    }

    pub async fn stop_output(&self) -> Result<()> {
        self.request(Command::StopOutput).await
    }

    pub fn stop_output_now(&self) {
        let (tx, _rx) = oneshot::channel();
        let _ = self.tx.send(Command::StopOutput(tx));
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) -> Result<()> {
        self.request(Command::Shutdown).await
    }

    pub fn shutdown_now(&self) {
        let (tx, _rx) = oneshot::channel();
        let _ = self.tx.send(Command::Shutdown(tx));
    }

    async fn request<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(make(tx))
            .map_err(|_| Error::new(ErrorKind::Disconnected, "actor", "actor thread is gone"))?;
        rx.await
            .map_err(|_| Error::new(ErrorKind::Cancelled, "actor", "actor dropped the reply"))?
    }

    fn request_blocking<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(make(tx))
            .map_err(|_| Error::new(ErrorKind::Disconnected, "actor", "actor thread is gone"))?;
        let mut rx = rx;
        loop {
            match rx.try_recv() {
                Ok(Some(value)) => return value,
                Ok(None) => thread::yield_now(),
                Err(oneshot::Canceled) => {
                    return Err(Error::new(ErrorKind::Cancelled, "actor", "actor dropped the reply"));
                }
            }
        }
    }
}

fn dispatch(backend: &mut dyn Backend, command: Command) -> bool {
    match command {
        Command::Enumerate(reply) => {
            let _ = reply.send(backend.enumerate());
            true
        }
        Command::Open(id, reply) => {
            let _ = reply.send(backend.open(&id));
            true
        }
        Command::DisplayModes(capture, reply) => {
            let _ = reply.send(backend.display_modes(capture));
            true
        }
        Command::EnableInput(config, sink, reply) => {
            let _ = reply.send(backend.enable_input(&config, sink));
            true
        }
        Command::StartInput(reply) => {
            let _ = reply.send(backend.start_input());
            true
        }
        Command::StopInput(reply) => {
            let _ = reply.send(backend.stop_input());
            true
        }
        Command::EnableOutput(config, sink, reply) => {
            let _ = reply.send(backend.enable_output(&config, sink));
            true
        }
        Command::ScheduleVideo(token, frame, reply) => {
            let _ = reply.send(backend.schedule_video(token, &frame));
            true
        }
        Command::ScheduleAudio(packet, reply) => {
            let _ = reply.send(backend.schedule_audio(&packet));
            true
        }
        Command::StopOutput(reply) => {
            let _ = reply.send(backend.stop_output());
            true
        }
        Command::Shutdown(reply) => {
            let _ = reply.send(backend.shutdown());
            false
        }
    }
}

impl Drop for ActorJoin {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
