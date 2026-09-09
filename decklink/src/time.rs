use std::num::NonZeroI64;

use crate::error::{Error, ErrorKind, Result};

/// DeckLink time value paired with its scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Time {
    pub value: i64,
    pub scale: NonZeroI64,
}

impl Time {
    pub fn new(value: i64, scale: i64) -> Result<Self> {
        let scale = NonZeroI64::new(scale)
            .ok_or_else(|| Error::new(ErrorKind::InvalidState, "time", "time scale must be non-zero"))?;
        Ok(Self { value, scale })
    }

    pub fn seconds(self) -> f64 {
        self.value as f64 / self.scale.get() as f64
    }

    pub fn rescale(self, new_scale: NonZeroI64) -> Self {
        if self.scale == new_scale {
            return self;
        }
        let value = (self.value as i128 * new_scale.get() as i128) / self.scale.get() as i128;
        Self {
            value: value as i64,
            scale: new_scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_scale() {
        assert!(Time::new(1, 0).is_err());
    }

    #[test]
    fn rescales_without_losing_ratio() {
        let time = Time::new(1001, 30_000).unwrap();
        let scaled = time.rescale(NonZeroI64::new(60_000).unwrap());
        assert_eq!(scaled.value, 2002);
    }
}
