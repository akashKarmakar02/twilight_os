//! `wlr-layer-shell-unstable-v1` protocol state and desktop placement.
//!
//! Protocol lifecycle/configure tracking lives here. Rendering remains in the
//! compositor, while [`layout`] contains the pure placement policy. This is the
//! same boundary Smithay uses, scaled down for twland's single-client server.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::{self, ErrorKind};

mod layout;
mod types;
pub mod wire;

pub use types::{
    Anchor, Configure, ExclusiveZone, Geometry, KeyboardInteractivity, Layer, LayerProperties,
    Margins, ProtocolError,
};

use crate::{Client, SurfaceRole, WaylandObjectKind};

#[derive(Debug, Default, Clone, Copy)]
pub struct DispatchEffects {
    pub redraw: bool,
    pub reconcile_keyboard_focus: bool,
    pub reconcile_pointer_focus: bool,
    pub layer_layout_changed: bool,
}

pub fn dispatch_shell_request(
    client: &mut Client,
    shell_id: u32,
    opcode: u16,
    payload: &[u8],
) -> io::Result<DispatchEffects> {
    match wire::parse_shell_request(opcode, payload)? {
        wire::ShellRequest::GetLayerSurface {
            id,
            surface_id,
            output_id,
            layer,
            namespace,
        } => {
            let layer = Layer::try_from(layer).map_err(protocol_error)?;
            if let Some(output_id) = output_id
                && !client
                    .objects
                    .get(&output_id)
                    .is_some_and(|object| object.kind == WaylandObjectKind::Output)
            {
                return Err(invalid_data(
                    "layer surface references an unknown wl_output",
                ));
            }

            let surface = client
                .surfaces
                .get(&surface_id)
                .ok_or_else(|| invalid_data("layer shell references an unknown wl_surface"))?;
            if surface.role.is_some() {
                return Err(invalid_data("wl_surface already has a role"));
            }
            if surface.has_committed
                || surface.attached_buffer.is_some()
                || surface.pending_buffer.is_some()
            {
                return Err(invalid_data(
                    "layer role assigned after wl_surface was constructed",
                ));
            }

            client.insert_object(id, WaylandObjectKind::LayerSurface)?;
            client.layer_shell.insert(LayerSurface::new(
                id, surface_id, output_id, layer, namespace,
            ));
            if let Some(surface) = client.surfaces.get_mut(&surface_id) {
                surface.role = Some(SurfaceRole::Layer(id));
            }

            let layer_surface = client.layer_shell.get(id).expect("inserted layer surface");
            println!(
                "twland: zwlr_layer_shell_v1.get_layer_surface id={id} surface={surface_id} output={:?} layer={layer:?} namespace=\"{}\"",
                layer_surface.output_id(),
                layer_surface.namespace()
            );
            Ok(DispatchEffects::default())
        }
        wire::ShellRequest::Destroy => {
            client.objects.remove(&shell_id);
            println!("twland: zwlr_layer_shell_v1.destroy id={shell_id}");
            Ok(DispatchEffects::default())
        }
        wire::ShellRequest::Unknown => {
            println!(
                "twland: zwlr_layer_shell_v1 request opcode={opcode} ignored object={shell_id}"
            );
            Ok(DispatchEffects::default())
        }
    }
}

pub fn dispatch_surface_request(
    client: &mut Client,
    layer_surface_id: u32,
    opcode: u16,
    payload: &[u8],
) -> io::Result<DispatchEffects> {
    match wire::parse_surface_request(opcode, payload)? {
        wire::SurfaceRequest::SetSize { width, height } => {
            let surface = get_surface_mut(client, layer_surface_id)?;
            surface.pending_mut().width = width;
            surface.pending_mut().height = height;
        }
        wire::SurfaceRequest::SetAnchor(bits) => {
            let anchor = Anchor::from_bits(bits).map_err(protocol_error)?;
            get_surface_mut(client, layer_surface_id)?
                .pending_mut()
                .anchor = anchor;
        }
        wire::SurfaceRequest::SetExclusiveZone(zone) => {
            get_surface_mut(client, layer_surface_id)?
                .pending_mut()
                .exclusive_zone = zone.into();
        }
        wire::SurfaceRequest::SetMargin {
            top,
            right,
            bottom,
            left,
        } => {
            get_surface_mut(client, layer_surface_id)?
                .pending_mut()
                .margins = Margins {
                top,
                right,
                bottom,
                left,
            };
        }
        wire::SurfaceRequest::SetKeyboardInteractivity(value) => {
            let interactivity = KeyboardInteractivity::try_from(value).map_err(protocol_error)?;
            get_surface_mut(client, layer_surface_id)?
                .pending_mut()
                .keyboard_interactivity = interactivity;
        }
        wire::SurfaceRequest::GetPopup => {
            println!(
                "twland: zwlr_layer_surface_v1.get_popup ignored; xdg_popup is not implemented"
            );
        }
        wire::SurfaceRequest::AckConfigure(serial) => {
            get_surface_mut(client, layer_surface_id)?
                .ack_configure(serial)
                .map_err(protocol_error)?;
            println!(
                "twland: zwlr_layer_surface_v1.ack_configure id={layer_surface_id} serial={serial}"
            );
        }
        wire::SurfaceRequest::Destroy => {
            client.objects.remove(&layer_surface_id);
            let Some(layer_surface) = client.layer_shell.remove(layer_surface_id) else {
                return Err(invalid_data("unknown zwlr_layer_surface_v1"));
            };
            if let Some(surface) = client.surfaces.get_mut(&layer_surface.wl_surface_id())
                && surface.role == Some(SurfaceRole::Layer(layer_surface_id))
            {
                surface.role = None;
                surface.mapped = false;
            }
            println!("twland: zwlr_layer_surface_v1.destroy id={layer_surface_id}");
            return Ok(DispatchEffects {
                redraw: true,
                reconcile_keyboard_focus: true,
                reconcile_pointer_focus: true,
                layer_layout_changed: true,
            });
        }
        wire::SurfaceRequest::SetLayer(value) => {
            let layer = Layer::try_from(value).map_err(protocol_error)?;
            get_surface_mut(client, layer_surface_id)?
                .pending_mut()
                .layer = layer;
        }
        wire::SurfaceRequest::Unknown => {
            println!(
                "twland: zwlr_layer_surface_v1 request opcode={opcode} ignored object={layer_surface_id}"
            );
        }
    }

    Ok(DispatchEffects::default())
}

fn get_surface_mut(client: &mut Client, layer_surface_id: u32) -> io::Result<&mut LayerSurface> {
    client
        .layer_shell
        .get_mut(layer_surface_id)
        .ok_or_else(|| invalid_data("unknown zwlr_layer_surface_v1"))
}

fn protocol_error(error: ProtocolError) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, error)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

#[derive(Debug)]
pub struct LayerSurface {
    id: u32,
    wl_surface_id: u32,
    output_id: Option<u32>,
    namespace: String,
    initial_layer: Layer,
    pending: LayerProperties,
    current: LayerProperties,
    initial_configure_sent: bool,
    pending_configures: Vec<Configure>,
    last_acked: Option<Configure>,
    mapped: bool,
    geometry: Option<Geometry>,
}

impl LayerSurface {
    pub fn new(
        id: u32,
        wl_surface_id: u32,
        output_id: Option<u32>,
        layer: Layer,
        namespace: String,
    ) -> Self {
        let properties = LayerProperties::initial(layer);
        Self {
            id,
            wl_surface_id,
            output_id,
            namespace,
            initial_layer: layer,
            pending: properties,
            current: properties,
            initial_configure_sent: false,
            pending_configures: Vec::new(),
            last_acked: None,
            mapped: false,
            geometry: None,
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn wl_surface_id(&self) -> u32 {
        self.wl_surface_id
    }

    pub fn output_id(&self) -> Option<u32> {
        self.output_id
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn current(&self) -> LayerProperties {
        self.current
    }

    pub fn pending_mut(&mut self) -> &mut LayerProperties {
        &mut self.pending
    }

    pub fn commit_pending(&mut self) -> Result<(), ProtocolError> {
        self.pending.validate()?;
        self.current = self.pending;
        Ok(())
    }

    pub fn needs_initial_configure(&self) -> bool {
        !self.initial_configure_sent
    }

    pub fn record_configure(&mut self, configure: Configure) {
        self.pending_configures.push(configure);
        self.initial_configure_sent = true;
    }

    pub fn ack_configure(&mut self, serial: u32) -> Result<Configure, ProtocolError> {
        let index = self
            .pending_configures
            .iter()
            .position(|configure| configure.serial == serial)
            .ok_or(ProtocolError::UnknownConfigure(serial))?;
        let configure = self.pending_configures[index];
        self.last_acked = Some(configure);
        self.pending_configures.drain(..=index);
        Ok(configure)
    }

    pub fn can_map(&self) -> bool {
        self.initial_configure_sent && self.last_acked.is_some()
    }

    pub fn mapped(&self) -> bool {
        self.mapped
    }

    pub fn map(&mut self) {
        self.mapped = true;
    }

    pub fn unmap(&mut self) {
        self.mapped = false;
        self.geometry = None;
        self.initial_configure_sent = false;
        self.pending_configures.clear();
        self.last_acked = None;
        let initial = LayerProperties::initial(self.initial_layer);
        self.pending = initial;
        self.current = initial;
    }

    pub fn geometry(&self) -> Option<Geometry> {
        self.geometry
    }

    fn set_geometry(&mut self, geometry: Geometry) {
        self.geometry = Some(geometry);
    }
}

#[derive(Debug)]
pub struct LayerShellState {
    surfaces: HashMap<u32, LayerSurface>,
    order: Vec<u32>,
    usable_area: Geometry,
}

impl LayerShellState {
    pub fn new(output: Geometry) -> Self {
        Self {
            surfaces: HashMap::new(),
            order: Vec::new(),
            usable_area: output,
        }
    }

    pub fn insert(&mut self, surface: LayerSurface) {
        self.order.push(surface.id());
        self.surfaces.insert(surface.id(), surface);
    }

    pub fn get(&self, id: u32) -> Option<&LayerSurface> {
        self.surfaces.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut LayerSurface> {
        self.surfaces.get_mut(&id)
    }

    pub fn remove(&mut self, id: u32) -> Option<LayerSurface> {
        self.order.retain(|candidate| *candidate != id);
        self.surfaces.remove(&id)
    }

    pub fn configure_size(&self, id: u32, output: Geometry) -> Option<(u32, u32)> {
        let surface = self.surfaces.get(&id)?;
        let source = match surface.current().exclusive_zone {
            ExclusiveZone::DontCare => output,
            ExclusiveZone::Exclusive(_) | ExclusiveZone::Neutral => self.usable_area,
        };
        Some(layout::configured_size(surface.current(), source))
    }

    pub fn arrange(&mut self, output: Geometry, buffer_sizes: &HashMap<u32, (i32, i32)>) {
        let mut usable = output;
        let mut exclusive = self.ordered_mapped_ids(true);
        let non_exclusive = self.ordered_mapped_ids(false);

        exclusive.sort_by_key(|id| {
            Reverse(
                self.surfaces
                    .get(id)
                    .expect("ordered layer surface must exist")
                    .current()
                    .layer,
            )
        });

        for id in exclusive.into_iter().chain(non_exclusive) {
            let Some(surface) = self.surfaces.get_mut(&id) else {
                continue;
            };
            let Some(buffer_size) = buffer_sizes.get(&surface.wl_surface_id()).copied() else {
                continue;
            };
            let properties = surface.current();
            let geometry = layout::place(properties, output, usable, buffer_size);
            surface.set_geometry(geometry);
            usable = layout::reserve(usable, properties);
        }

        self.usable_area = usable;
    }

    pub fn usable_area(&self) -> Geometry {
        self.usable_area
    }

    pub fn surface_ids_on(&self, layer: Layer) -> Vec<u32> {
        self.order
            .iter()
            .filter_map(|id| {
                self.surfaces
                    .get(id)
                    .filter(|surface| surface.mapped() && surface.current().layer == layer)
                    .map(LayerSurface::wl_surface_id)
            })
            .collect()
    }

    pub fn topmost_at_on(&self, layer: Layer, x: i32, y: i32) -> Option<u32> {
        self.order.iter().rev().find_map(|id| {
            let surface = self.surfaces.get(id)?;
            (surface.mapped()
                && surface.current().layer == layer
                && surface
                    .geometry()
                    .is_some_and(|geometry| geometry.contains(x, y)))
            .then_some(surface.wl_surface_id())
        })
    }

    pub fn exclusive_keyboard_focus(&self) -> Option<u32> {
        [Layer::Overlay, Layer::Top].into_iter().find_map(|layer| {
            self.order.iter().rev().find_map(|id| {
                let surface = self.surfaces.get(id)?;
                (surface.mapped()
                    && surface.current().layer == layer
                    && surface.current().keyboard_interactivity == KeyboardInteractivity::Exclusive)
                    .then_some(surface.wl_surface_id())
            })
        })
    }

    pub fn keyboard_interactivity_for_surface(
        &self,
        wl_surface_id: u32,
    ) -> Option<KeyboardInteractivity> {
        self.surfaces
            .values()
            .find(|surface| surface.wl_surface_id() == wl_surface_id)
            .map(|surface| surface.current().keyboard_interactivity)
    }

    pub fn can_keep_keyboard_focus(&self, wl_surface_id: u32) -> bool {
        self.surfaces.values().any(|surface| {
            surface.wl_surface_id() == wl_surface_id
                && surface.mapped()
                && surface.current().keyboard_interactivity != KeyboardInteractivity::None
        })
    }

    pub fn geometry_for_surface(&self, wl_surface_id: u32) -> Option<Geometry> {
        self.surfaces
            .values()
            .find(|surface| surface.wl_surface_id() == wl_surface_id)
            .and_then(LayerSurface::geometry)
    }

    fn ordered_mapped_ids(&self, exclusive: bool) -> Vec<u32> {
        self.order
            .iter()
            .copied()
            .filter(|id| {
                self.surfaces.get(id).is_some_and(|surface| {
                    surface.mapped()
                        && matches!(
                            surface.current().exclusive_zone,
                            ExclusiveZone::Exclusive(_)
                        ) == exclusive
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        Anchor, Configure, ExclusiveZone, Geometry, Layer, LayerShellState, LayerSurface, Margins,
    };

    const OUTPUT: Geometry = Geometry {
        x: 0,
        y: 0,
        width: 800,
        height: 600,
    };

    #[test]
    fn configure_ack_is_required_again_after_unmap() {
        let mut surface = LayerSurface::new(10, 20, None, Layer::Top, "panel".into());
        surface.pending_mut().width = 800;
        surface.pending_mut().height = 30;
        surface.commit_pending().unwrap();
        surface.record_configure(Configure {
            serial: 1,
            width: 800,
            height: 30,
        });
        surface.ack_configure(1).unwrap();
        assert!(surface.can_map());

        surface.map();
        surface.unmap();
        assert!(!surface.can_map());
        assert!(surface.needs_initial_configure());
        assert_eq!(surface.current().layer, Layer::Top);
        assert_eq!(surface.current().width, 0);
        assert!(matches!(
            surface.ack_configure(1),
            Err(super::ProtocolError::UnknownConfigure(1))
        ));
    }

    #[test]
    fn acknowledging_a_configure_retires_older_events_by_send_order() {
        let mut surface = LayerSurface::new(10, 20, None, Layer::Top, "panel".into());
        surface.record_configure(Configure {
            serial: u32::MAX,
            width: 800,
            height: 30,
        });
        surface.record_configure(Configure {
            serial: 0,
            width: 700,
            height: 30,
        });

        assert_eq!(surface.ack_configure(0).unwrap().width, 700);
        assert!(matches!(
            surface.ack_configure(u32::MAX),
            Err(super::ProtocolError::UnknownConfigure(u32::MAX))
        ));
    }

    #[test]
    fn exclusive_panel_updates_usable_area() {
        let mut state = LayerShellState::new(OUTPUT);
        let mut panel = LayerSurface::new(10, 20, None, Layer::Top, "panel".into());
        panel.pending_mut().width = 800;
        panel.pending_mut().height = 30;
        panel.pending_mut().anchor = Anchor::from_bits(13).unwrap();
        panel.pending_mut().exclusive_zone = ExclusiveZone::Exclusive(30);
        panel.pending_mut().margins = Margins {
            top: 5,
            ..Margins::default()
        };
        panel.commit_pending().unwrap();
        panel.map();
        state.insert(panel);

        state.arrange(OUTPUT, &HashMap::from([(20, (800, 30))]));
        assert_eq!(
            state.usable_area(),
            Geometry {
                x: 0,
                y: 35,
                width: 800,
                height: 565,
            }
        );
    }

    #[test]
    fn exclusive_surfaces_are_arranged_from_overlay_to_background() {
        let mut state = LayerShellState::new(OUTPUT);

        let mut bottom = LayerSurface::new(10, 20, None, Layer::Bottom, "bottom".into());
        bottom.pending_mut().width = 800;
        bottom.pending_mut().height = 20;
        bottom.pending_mut().anchor = Anchor::from_bits(13).unwrap();
        bottom.pending_mut().exclusive_zone = ExclusiveZone::Exclusive(20);
        bottom.commit_pending().unwrap();
        bottom.map();
        state.insert(bottom);

        let mut overlay = LayerSurface::new(11, 21, None, Layer::Overlay, "overlay".into());
        overlay.pending_mut().width = 800;
        overlay.pending_mut().height = 30;
        overlay.pending_mut().anchor = Anchor::from_bits(13).unwrap();
        overlay.pending_mut().exclusive_zone = ExclusiveZone::Exclusive(30);
        overlay.commit_pending().unwrap();
        overlay.map();
        state.insert(overlay);

        state.arrange(OUTPUT, &HashMap::from([(20, (800, 20)), (21, (800, 30))]));

        assert_eq!(state.geometry_for_surface(21).unwrap().y, 0);
        assert_eq!(state.geometry_for_surface(20).unwrap().y, 30);
        assert_eq!(state.usable_area().y, 50);
    }
}
