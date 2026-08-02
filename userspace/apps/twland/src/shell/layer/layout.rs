use super::types::{Anchor, ExclusiveZone, Geometry, LayerProperties};

pub fn configured_size(properties: LayerProperties, source: Geometry) -> (u32, u32) {
    let source = margin_adjusted_source(properties, source);
    let mut width = (properties.width.min(i32::MAX as u32) as i32)
        .min(source.width)
        .max(0);
    let mut height = (properties.height.min(i32::MAX as u32) as i32)
        .min(source.height)
        .max(0);

    if properties.anchor.anchored_horizontally() {
        width = source.width;
    }
    if properties.anchor.anchored_vertically() {
        height = source.height;
    }

    (width as u32, height as u32)
}

pub fn place(
    properties: LayerProperties,
    output: Geometry,
    usable: Geometry,
    buffer_size: (i32, i32),
) -> Geometry {
    let base = match properties.exclusive_zone {
        ExclusiveZone::DontCare => output,
        ExclusiveZone::Exclusive(_) | ExclusiveZone::Neutral => usable,
    };
    let source = margin_adjusted_source(properties, base);
    let width = buffer_size.0.max(0);
    let height = buffer_size.1.max(0);

    let x = if properties.anchor.anchored_horizontally() {
        source
            .x
            .saturating_add(source.width.saturating_sub(width) / 2)
    } else if properties.anchor.contains(Anchor::LEFT) {
        source.x
    } else if properties.anchor.contains(Anchor::RIGHT) {
        source.x.saturating_add(source.width.saturating_sub(width))
    } else {
        source
            .x
            .saturating_add(source.width.saturating_sub(width) / 2)
    };
    let y = if properties.anchor.anchored_vertically() {
        source
            .y
            .saturating_add(source.height.saturating_sub(height) / 2)
    } else if properties.anchor.contains(Anchor::TOP) {
        source.y
    } else if properties.anchor.contains(Anchor::BOTTOM) {
        source
            .y
            .saturating_add(source.height.saturating_sub(height))
    } else {
        source
            .y
            .saturating_add(source.height.saturating_sub(height) / 2)
    };

    Geometry {
        x,
        y,
        width,
        height,
    }
}

pub fn reserve(mut usable: Geometry, properties: LayerProperties) -> Geometry {
    let ExclusiveZone::Exclusive(amount) = properties.exclusive_zone else {
        return usable;
    };
    let Some(edge) = properties.anchor.effective_exclusive_edge() else {
        return usable;
    };

    let amount = amount.min(i32::MAX as u32) as i32;
    if edge == Anchor::TOP {
        let reserved = amount
            .saturating_add(properties.margins.top)
            .max(0)
            .min(usable.height);
        usable.y = usable.y.saturating_add(reserved);
        usable.height -= reserved;
    } else if edge == Anchor::BOTTOM {
        let reserved = amount
            .saturating_add(properties.margins.bottom)
            .max(0)
            .min(usable.height);
        usable.height -= reserved;
    } else if edge == Anchor::LEFT {
        let reserved = amount
            .saturating_add(properties.margins.left)
            .max(0)
            .min(usable.width);
        usable.x = usable.x.saturating_add(reserved);
        usable.width -= reserved;
    } else if edge == Anchor::RIGHT {
        let reserved = amount
            .saturating_add(properties.margins.right)
            .max(0)
            .min(usable.width);
        usable.width -= reserved;
    }
    usable
}

fn margin_adjusted_source(properties: LayerProperties, mut source: Geometry) -> Geometry {
    if properties.anchor.contains(Anchor::LEFT) {
        source.x = source.x.saturating_add(properties.margins.left);
        source.width = source.width.saturating_sub(properties.margins.left);
    }
    if properties.anchor.contains(Anchor::RIGHT) {
        source.width = source.width.saturating_sub(properties.margins.right);
    }
    if properties.anchor.contains(Anchor::TOP) {
        source.y = source.y.saturating_add(properties.margins.top);
        source.height = source.height.saturating_sub(properties.margins.top);
    }
    if properties.anchor.contains(Anchor::BOTTOM) {
        source.height = source.height.saturating_sub(properties.margins.bottom);
    }
    source.width = source.width.max(0);
    source.height = source.height.max(0);
    source
}

#[cfg(test)]
mod tests {
    use super::{configured_size, place, reserve};
    use crate::shell::layer::{Anchor, ExclusiveZone, Geometry, Layer, LayerProperties, Margins};

    const OUTPUT: Geometry = Geometry {
        x: 0,
        y: 0,
        width: 800,
        height: 600,
    };

    #[test]
    fn opposite_anchors_fill_the_configured_axis() {
        let mut properties = LayerProperties::initial(Layer::Background);
        properties.anchor = Anchor::from_bits(15).unwrap();
        assert_eq!(configured_size(properties, OUTPUT), (800, 600));
    }

    #[test]
    fn top_panel_reserves_usable_area_including_margin() {
        let mut properties = LayerProperties::initial(Layer::Top);
        properties.width = 800;
        properties.height = 30;
        properties.anchor = Anchor::from_bits(13).unwrap();
        properties.exclusive_zone = ExclusiveZone::Exclusive(30);
        properties.margins = Margins {
            top: 5,
            ..Margins::default()
        };

        assert_eq!(
            reserve(OUTPUT, properties),
            Geometry {
                x: 0,
                y: 35,
                width: 800,
                height: 565,
            }
        );
    }

    #[test]
    fn anchored_surface_is_placed_against_requested_edge() {
        let mut properties = LayerProperties::initial(Layer::Top);
        properties.width = 200;
        properties.height = 40;
        properties.anchor = Anchor::TOP;
        let geometry = place(properties, OUTPUT, OUTPUT, (200, 40));
        assert_eq!(geometry.x, 300);
        assert_eq!(geometry.y, 0);
    }

    #[test]
    fn right_anchor_uses_the_committed_surface_size() {
        let mut properties = LayerProperties::initial(Layer::Top);
        properties.width = 300;
        properties.height = 40;
        properties.anchor = Anchor::RIGHT;

        let geometry = place(properties, OUTPUT, OUTPUT, (200, 40));
        assert_eq!(geometry.x, 600);
    }
}
