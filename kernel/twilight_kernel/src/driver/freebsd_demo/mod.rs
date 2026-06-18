use crate::compat::freebsd_kpi::bus_space::bus_space_read_1;
use crate::compat::freebsd_kpi::callout::{
    Callout, callout_active, callout_drain, callout_init, callout_initialized, callout_pending,
    callout_reset, callout_stop,
};
use crate::compat::freebsd_kpi::device::{
    Device, device_get_device, device_get_nameunit, device_get_vendor, device_set_desc,
};
use crate::compat::freebsd_kpi::dma::{
    BUS_DMA_NOWAIT, BUS_DMA_ZERO, BUS_DMASYNC_PREREAD, BUS_DMASYNC_PREWRITE, BUS_SPACE_MAXADDR,
    BUS_SPACE_MAXADDR_32BIT, BusDmaSegment, bus_dma_tag_create, bus_dma_tag_destroy,
    bus_dmamap_load, bus_dmamap_sync, bus_dmamap_unload, bus_dmamem_alloc, bus_dmamem_free,
};
use crate::compat::freebsd_kpi::driver::{BUS_PROBE_DEFAULT, ENXIO, FreeBsdPciDriver};
use crate::compat::freebsd_kpi::intr::{
    INTR_MPSAFE, INTR_TYPE_NET, IntrCookie, bus_setup_intr, bus_teardown_intr,
    debug_registered_intr_count,
};
use crate::compat::freebsd_kpi::mutex::{
    MTX_DEF, MTX_NOWITNESS, Mtx, mtx_destroy, mtx_init, mtx_initialized, mtx_lock, mtx_unlock,
};
use crate::compat::freebsd_kpi::pci::{pci_get_device, pci_get_vendor};
use crate::compat::freebsd_kpi::resource::{
    RF_ACTIVE, Resource, SYS_RES_IOPORT, SYS_RES_IRQ, bus_alloc_resource_any, rman_get_bushandle,
    rman_get_bustag, rman_get_start,
};
use crate::compat::freebsd_kpi::taskqueue::{
    Task, task_init, taskqueue_create, taskqueue_drain, taskqueue_enqueue, taskqueue_free,
    taskqueue_len, taskqueue_run,
};
use crate::sys::pci::{PciClaimError, PciOwner, PciOwnerKind};
use crate::{log, sys};

const RTL8139_VENDOR_ID: u16 = 0x10EC;
const RTL8139_DEVICE_ID: u16 = 0x8139;
const RTL8139_DESC: &str = "FreeBSD KPI demo RTL8139";
const FREEBSD_DEMO_CLAIM_RTL8139: bool = false;

struct Rtl8139DemoDriver;

struct Rtl8139DemoSoftc {
    io_base: u64,
    irq: u8,
    intr_cookie: Option<IntrCookie>,
    lock: Mtx,
}

impl Rtl8139DemoSoftc {
    fn new() -> Self {
        Self {
            io_base: 0,
            irq: 0,
            intr_cookie: None,
            lock: Mtx::new(),
        }
    }
}

impl FreeBsdPciDriver for Rtl8139DemoDriver {
    fn probe(&mut self, device: &mut Device) -> i32 {
        if pci_get_vendor(device) == RTL8139_VENDOR_ID
            && pci_get_device(device) == RTL8139_DEVICE_ID
        {
            device_set_desc(device, RTL8139_DESC);
            log!(
                "freebsd_kpi_demo: probe {} {:04x}:{:04x}",
                device_get_nameunit(device),
                device_get_vendor(device),
                device_get_device(device)
            );
            return BUS_PROBE_DEFAULT;
        }

        ENXIO
    }

    fn attach(&mut self, device: &mut Device) -> i32 {
        let id = device.id();
        log!(
            "freebsd_kpi_demo: attach {} {:02x}:{:02x}.{} {:04x}:{:04x} {}",
            device_get_nameunit(device),
            id.bus,
            id.slot,
            id.function,
            device_get_vendor(device),
            device_get_device(device),
            device.desc().unwrap_or("unknown device")
        );
        log_rtl8139_resources(device);
        0
    }

    fn detach(&mut self, _device: &mut Device) -> i32 {
        0
    }
}

pub fn init() {
    let mut driver = Rtl8139DemoDriver;
    let mut matched = false;

    for pci_config in sys::pci::list() {
        let mut device = Device::from_pci_config(pci_config);
        if driver.probe(&mut device) == BUS_PROBE_DEFAULT {
            matched = true;
            if !FREEBSD_DEMO_CLAIM_RTL8139 {
                log!(
                    "freebsd_kpi_demo: probe-only, not claiming {} {:04x}:{:04x}",
                    device_get_nameunit(&device),
                    device_get_vendor(&device),
                    device_get_device(&device)
                );
                continue;
            }

            let id = device.id();
            let owner = PciOwner {
                kind: PciOwnerKind::FreeBsdKpiDriver,
                name: "freebsd_kpi_demo",
            };

            match sys::pci::claim_device(id.bus, id.slot, id.function, owner) {
                Ok(()) => {
                    log!(
                        "freebsd_kpi_demo: claimed {} {:04x}:{:04x}",
                        device_get_nameunit(&device),
                        device_get_vendor(&device),
                        device_get_device(&device)
                    );

                    let result = driver.attach(&mut device);
                    if result != 0 {
                        log!("freebsd_kpi_demo: attach failed with {}", result);
                    }
                }
                Err(PciClaimError::AlreadyClaimed(owner)) => {
                    log!(
                        "freebsd_kpi_demo: attach skipped: {} {:04x}:{:04x} already claimed by {}/{}",
                        device_get_nameunit(&device),
                        device_get_vendor(&device),
                        device_get_device(&device),
                        owner.kind.as_str(),
                        owner.name
                    );
                }
                Err(PciClaimError::NotFound) => {
                    log!(
                        "freebsd_kpi_demo: attach skipped: {} {:04x}:{:04x} disappeared before claim",
                        device_get_nameunit(&device),
                        device_get_vendor(&device),
                        device_get_device(&device)
                    );
                }
            }
        }
    }

    if !matched {
        log!("freebsd_kpi_demo: no matching RTL8139 device found");
    }
}

fn log_rtl8139_resources(device: &Device) {
    let Some(io_resource) = bus_alloc_resource_any(device, SYS_RES_IOPORT, 0, RF_ACTIVE) else {
        log!("freebsd_kpi_demo: no RTL8139 I/O BAR0 resource");
        return;
    };

    let Some(irq_resource) = bus_alloc_resource_any(device, SYS_RES_IRQ, 0, RF_ACTIVE) else {
        log!("freebsd_kpi_demo: no RTL8139 IRQ resource");
        return;
    };

    let mut softc = Rtl8139DemoSoftc::new();
    log!("freebsd_kpi_demo: softc initialized");

    mtx_init(
        &mut softc.lock,
        "freebsd_kpi_demo",
        Some("rtl8139_demo"),
        MTX_DEF | MTX_NOWITNESS,
    );
    if mtx_initialized(&softc.lock) {
        log!("freebsd_kpi_demo: mtx initialized");
    }

    let io_base = rman_get_start(io_resource);
    let irq = rman_get_start(irq_resource) as u8;
    mtx_lock(&softc.lock);
    softc.io_base = io_base;
    softc.irq = irq;
    mtx_unlock(&softc.lock);

    log!(
        "freebsd_kpi_demo: stored io_base={:#x} irq={}",
        softc.io_base,
        softc.irq
    );
    log!("freebsd_kpi_demo: i/o bar0 base={:#x}", softc.io_base);
    log!("freebsd_kpi_demo: irq={}", softc.irq);

    let tag = rman_get_bustag(io_resource);
    let handle = rman_get_bushandle(io_resource);
    let mac = [
        bus_space_read_1(tag, handle, 0x00),
        bus_space_read_1(tag, handle, 0x01),
        bus_space_read_1(tag, handle, 0x02),
        bus_space_read_1(tag, handle, 0x03),
        bus_space_read_1(tag, handle, 0x04),
        bus_space_read_1(tag, handle, 0x05),
    ];

    log!(
        "freebsd_kpi_demo: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    register_dummy_intr(device, irq_resource, &mut softc);
    exercise_callout();
    exercise_taskqueue();
    exercise_dma();
    mtx_destroy(&mut softc.lock);
    log!("freebsd_kpi_demo: mtx destroyed");
}

fn register_dummy_intr(device: &Device, irq_resource: Resource, softc: &mut Rtl8139DemoSoftc) {
    let mut cookie: Option<IntrCookie> = None;
    let result = bus_setup_intr(
        device,
        irq_resource,
        INTR_TYPE_NET | INTR_MPSAFE,
        None,
        Some(rtl8139_demo_intr),
        0,
        &mut cookie,
    );

    if result != 0 {
        log!(
            "freebsd_kpi_demo: dummy irq registration failed: {}",
            result
        );
        return;
    }

    let Some(cookie) = cookie else {
        log!("freebsd_kpi_demo: dummy irq registration returned no cookie");
        return;
    };

    mtx_lock(&softc.lock);
    softc.intr_cookie = Some(cookie);
    mtx_unlock(&softc.lock);

    log!(
        "freebsd_kpi_demo: dummy irq cookie={} registered_count={}",
        cookie.id(),
        debug_registered_intr_count()
    );

    mtx_lock(&softc.lock);
    let cookie = softc.intr_cookie;
    mtx_unlock(&softc.lock);

    let Some(cookie) = cookie else {
        log!("freebsd_kpi_demo: dummy irq teardown skipped: no softc cookie");
        return;
    };

    let result = bus_teardown_intr(device, irq_resource, cookie);
    if result == 0 {
        mtx_lock(&softc.lock);
        softc.intr_cookie = None;
        mtx_unlock(&softc.lock);
        log!(
            "freebsd_kpi_demo: dummy irq handler removed registered_count={}",
            debug_registered_intr_count()
        );
    } else {
        log!("freebsd_kpi_demo: dummy irq teardown failed: {}", result);
    }
}

fn rtl8139_demo_intr(_arg: usize) {
    log!("freebsd_kpi_demo: dummy interrupt handler called");
}

fn exercise_callout() {
    let mut callout = Callout::new();
    callout_init(&mut callout, true);

    if callout_initialized(&callout) {
        log!("freebsd_kpi_demo: callout initialized");
    }

    let result = callout_reset(&mut callout, 10, rtl8139_demo_callout, 0);
    if result == 0 {
        log!("freebsd_kpi_demo: callout armed");
    } else {
        log!("freebsd_kpi_demo: callout arm failed: {}", result);
        return;
    }

    log!(
        "freebsd_kpi_demo: callout pending={} active={}",
        callout_pending(&callout),
        callout_active(&callout)
    );

    let stopped = callout_stop(&mut callout);
    log!("freebsd_kpi_demo: callout stopped result={}", stopped);

    let drained = callout_drain(&mut callout);
    log!("freebsd_kpi_demo: callout drained result={}", drained);
}

fn exercise_taskqueue() {
    let mut task = Task::new();
    let mut queue = taskqueue_create("freebsd_kpi_demo");
    log!("freebsd_kpi_demo: taskqueue created");

    task_init(&mut task, 0, rtl8139_demo_task, 0);
    let result = taskqueue_enqueue(&mut queue, &mut task);
    if result != 0 {
        log!("freebsd_kpi_demo: task enqueue failed: {}", result);
        taskqueue_free(queue);
        return;
    }

    log!(
        "freebsd_kpi_demo: task enqueued queue_len={}",
        taskqueue_len(&queue)
    );
    taskqueue_run(&mut queue);
    log!("freebsd_kpi_demo: taskqueue run");
    taskqueue_drain(&mut queue, &mut task);
    log!("freebsd_kpi_demo: taskqueue drained");
    taskqueue_free(queue);
    log!("freebsd_kpi_demo: taskqueue freed");
}

fn rtl8139_demo_callout(_arg: usize) {
    log!("freebsd_kpi_demo: dummy callout fired");
}

fn rtl8139_demo_task(_context: usize, pending: i32) {
    log!("freebsd_kpi_demo: dummy task ran pending={}", pending);
}

fn exercise_dma() {
    let tag = match bus_dma_tag_create(
        16,
        0,
        BUS_SPACE_MAXADDR_32BIT,
        BUS_SPACE_MAXADDR,
        4096,
        1,
        4096,
        0,
    ) {
        Ok(tag) => {
            log!("freebsd_kpi_demo: dma tag created");
            tag
        }
        Err(error) => {
            log!("freebsd_kpi_demo: dma tag create failed: {}", error);
            return;
        }
    };

    let (vaddr, mut map) = match bus_dmamem_alloc(&tag, BUS_DMA_NOWAIT | BUS_DMA_ZERO) {
        Ok((vaddr, map)) => {
            log!(
                "freebsd_kpi_demo: dma memory allocated vaddr={:#x} paddr={:#x} size={}",
                vaddr,
                map.paddr,
                map.size
            );
            (vaddr, map)
        }
        Err(error) => {
            log!("freebsd_kpi_demo: dma memory allocation failed: {}", error);
            let _ = bus_dma_tag_destroy(tag);
            return;
        }
    };

    let size = map.size;
    let result = bus_dmamap_load(&tag, &mut map, vaddr, size, rtl8139_demo_dma_callback, 0);
    if result != 0 {
        log!("freebsd_kpi_demo: dma map load failed: {}", result);
        let _ = bus_dmamem_free(&tag, vaddr, map);
        let _ = bus_dma_tag_destroy(tag);
        return;
    }
    log!("freebsd_kpi_demo: dma map loaded");

    bus_dmamap_sync(&tag, &map, BUS_DMASYNC_PREREAD);
    bus_dmamap_sync(&tag, &map, BUS_DMASYNC_PREWRITE);
    bus_dmamap_unload(&tag, &mut map);
    log!("freebsd_kpi_demo: dma map unloaded");

    let result = bus_dmamem_free(&tag, vaddr, map);
    if result == 0 {
        log!("freebsd_kpi_demo: dma memory freed");
    } else {
        log!("freebsd_kpi_demo: dma memory free failed: {}", result);
    }

    let result = bus_dma_tag_destroy(tag);
    if result == 0 {
        log!("freebsd_kpi_demo: dma tag destroyed");
    }
}

fn rtl8139_demo_dma_callback(_callback_arg: usize, segs: &[BusDmaSegment], error: i32) {
    if error != 0 {
        log!("freebsd_kpi_demo: dma callback error={}", error);
        return;
    }

    if let Some(seg) = segs.first() {
        log!(
            "freebsd_kpi_demo: dma callback segment addr={:#x} len={}",
            seg.ds_addr,
            seg.ds_len
        );
    }
}
