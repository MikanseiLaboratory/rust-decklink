use std::sync::{Arc, Mutex, OnceLock};

use crate::actor::{ActorHandle, ActorJoin};
use crate::backend::Backend;
use crate::backend::mock::{MockBackend, MockWorld};
use crate::device::Device;
use crate::error::{Error, ErrorKind, Result};
use crate::mode::DisplayMode;

struct SharedContext {
    actor: ActorHandle,
    hardware: bool,
    _join: Mutex<Option<ActorJoin>>,
}

/// Process-wide DeckLink entry point. One actor thread owns COM objects.
#[derive(Clone)]
pub struct DeckLinkContext {
    inner: Arc<SharedContext>,
}

static HARDWARE: OnceLock<std::result::Result<DeckLinkContext, Error>> = OnceLock::new();

impl DeckLinkContext {
    /// Connect to installed DeckLink drivers. Falls back to a clear error when
    /// the hardware shim is not available.
    pub fn new() -> Result<Self> {
        HARDWARE
            .get_or_init(|| {
                let (actor, join) = ActorHandle::spawn(|| {
                    Ok(Box::new(crate::backend::hardware::HardwareBackend::new()?) as Box<dyn Backend>)
                })?;
                Ok(Self {
                    inner: Arc::new(SharedContext {
                        actor,
                        hardware: true,
                        _join: Mutex::new(Some(join)),
                    }),
                })
            })
            .clone()
    }

    /// In-memory devices for tests and examples that must run without a card.
    pub fn mock(world: MockWorld) -> Self {
        let (actor, join) =
            ActorHandle::spawn(move || Ok(Box::new(MockBackend::new(world)) as Box<dyn Backend>)).expect("mock actor");
        Self {
            inner: Arc::new(SharedContext {
                actor,
                hardware: false,
                _join: Mutex::new(Some(join)),
            }),
        }
    }

    /// Try hardware, then the built-in demo mock.
    pub fn connect() -> Self {
        Self::new().unwrap_or_else(|_| Self::mock(MockWorld::demo()))
    }

    pub fn is_hardware(&self) -> bool {
        self.inner.hardware
    }

    pub fn api_version(&self) -> Result<String> {
        if self.inner.hardware {
            decklink_sys::api_version().map_err(|hr| Error::sdk("api_version", crate::error::Hresult(hr)))
        } else {
            Ok("mock-16.0".into())
        }
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let snapshots = self.inner.actor.enumerate_blocking()?;
        Ok(snapshots
            .into_iter()
            .enumerate()
            .map(|(index, snapshot)| Device {
                info: snapshot.info,
                index,
                actor: self.inner.actor.clone(),
            })
            .collect())
    }

    pub fn first_device(&self) -> Result<Device> {
        self.devices()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::new(ErrorKind::DriverNotFound, "devices", "no DeckLink devices"))
    }
}

pub(crate) fn actor_for(device: &Device) -> Result<ActorHandle> {
    Ok(device.actor.clone())
}

pub(crate) fn list_display_modes(device: &Device) -> Result<Vec<DisplayMode>> {
    let actor = actor_for(device)?;
    actor.open_blocking(device.info.id.clone())?;
    actor.display_modes_blocking(device.info.supports_capture)
}

impl Drop for SharedContext {
    fn drop(&mut self) {
        self.actor.shutdown_now();
        if let Ok(mut join) = self._join.lock() {
            drop(join.take());
        }
    }
}
