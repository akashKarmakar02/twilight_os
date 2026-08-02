use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

impl TryFrom<u32> for Layer {
    type Error = ProtocolError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Background),
            1 => Ok(Self::Bottom),
            2 => Ok(Self::Top),
            3 => Ok(Self::Overlay),
            _ => Err(ProtocolError::InvalidLayer(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor(u32);

impl Anchor {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const BOTTOM: Self = Self(2);
    pub const LEFT: Self = Self(4);
    pub const RIGHT: Self = Self(8);
    const ALL_BITS: u32 = Self::TOP.0 | Self::BOTTOM.0 | Self::LEFT.0 | Self::RIGHT.0;

    pub fn from_bits(bits: u32) -> Result<Self, ProtocolError> {
        if bits & !Self::ALL_BITS == 0 {
            Ok(Self(bits))
        } else {
            Err(ProtocolError::InvalidAnchor(bits))
        }
    }

    pub fn contains(self, edge: Self) -> bool {
        self.0 & edge.0 == edge.0
    }

    pub fn anchored_horizontally(self) -> bool {
        self.contains(Self::LEFT) && self.contains(Self::RIGHT)
    }

    pub fn anchored_vertically(self) -> bool {
        self.contains(Self::TOP) && self.contains(Self::BOTTOM)
    }

    pub fn effective_exclusive_edge(self) -> Option<Self> {
        match self.0.count_ones() {
            1 => Some(self),
            3 => {
                if !self.contains(Self::TOP) {
                    Some(Self::BOTTOM)
                } else if !self.contains(Self::BOTTOM) {
                    Some(Self::TOP)
                } else if !self.contains(Self::LEFT) {
                    Some(Self::RIGHT)
                } else {
                    Some(Self::LEFT)
                }
            }
            _ => None,
        }
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::NONE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardInteractivity {
    None,
    Exclusive,
    OnDemand,
}

impl TryFrom<u32> for KeyboardInteractivity {
    type Error = ProtocolError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Exclusive),
            2 => Ok(Self::OnDemand),
            _ => Err(ProtocolError::InvalidKeyboardInteractivity(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusiveZone {
    Exclusive(u32),
    Neutral,
    DontCare,
}

impl From<i32> for ExclusiveZone {
    fn from(value: i32) -> Self {
        match value {
            value if value > 0 => Self::Exclusive(value as u32),
            0 => Self::Neutral,
            _ => Self::DontCare,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Margins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerProperties {
    pub width: u32,
    pub height: u32,
    pub anchor: Anchor,
    pub exclusive_zone: ExclusiveZone,
    pub margins: Margins,
    pub keyboard_interactivity: KeyboardInteractivity,
    pub layer: Layer,
}

impl LayerProperties {
    pub fn initial(layer: Layer) -> Self {
        Self {
            width: 0,
            height: 0,
            anchor: Anchor::default(),
            exclusive_zone: ExclusiveZone::Neutral,
            margins: Margins::default(),
            keyboard_interactivity: KeyboardInteractivity::None,
            layer,
        }
    }

    pub fn validate(self) -> Result<(), ProtocolError> {
        if self.width == 0 && !self.anchor.anchored_horizontally() {
            return Err(ProtocolError::InvalidSize(
                "width 0 requires left and right anchors",
            ));
        }
        if self.height == 0 && !self.anchor.anchored_vertically() {
            return Err(ProtocolError::InvalidSize(
                "height 0 requires top and bottom anchors",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Geometry {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Configure {
    pub serial: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidLayer(u32),
    InvalidAnchor(u32),
    InvalidKeyboardInteractivity(u32),
    InvalidSize(&'static str),
    UnknownConfigure(u32),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLayer(value) => write!(formatter, "invalid layer {value}"),
            Self::InvalidAnchor(value) => write!(formatter, "invalid anchor bits 0x{value:x}"),
            Self::InvalidKeyboardInteractivity(value) => {
                write!(formatter, "invalid keyboard interactivity {value}")
            }
            Self::InvalidSize(message) => formatter.write_str(message),
            Self::UnknownConfigure(serial) => {
                write!(formatter, "unknown configure serial {serial}")
            }
        }
    }
}

impl Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::{Anchor, Layer, LayerProperties, ProtocolError};

    #[test]
    fn exclusive_edge_is_only_implied_by_one_or_three_anchors() {
        assert_eq!(Anchor::NONE.effective_exclusive_edge(), None);
        assert_eq!(Anchor::TOP.effective_exclusive_edge(), Some(Anchor::TOP));
        assert_eq!(
            Anchor::from_bits(Anchor::TOP.0 | Anchor::LEFT.0)
                .unwrap()
                .effective_exclusive_edge(),
            None
        );
        assert_eq!(
            Anchor::from_bits(Anchor::TOP.0 | Anchor::LEFT.0 | Anchor::RIGHT.0)
                .unwrap()
                .effective_exclusive_edge(),
            Some(Anchor::TOP)
        );
    }

    #[test]
    fn zero_size_requires_opposite_anchors() {
        let mut properties = LayerProperties::initial(Layer::Background);
        assert!(matches!(
            properties.validate(),
            Err(ProtocolError::InvalidSize(_))
        ));

        properties.anchor = Anchor::from_bits(15).unwrap();
        assert_eq!(properties.validate(), Ok(()));
    }
}
