use crate::driver::timer::cmos::CMOS;
use crate::println;
use crate::task::executor::sleep;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::SocketSet;
use smoltcp::socket::dhcpv4;
use smoltcp::socket::dhcpv4::Event;
use smoltcp::time::Instant;

pub fn main() {
    let mut dhcp_config = None;

    if let Some((ref mut iface, ref mut device)) = *crate::driver::nic::NET.lock() {
        let dhcp_socket = dhcpv4::Socket::new();
        let mut sockets = SocketSet::new(vec![]);
        let dhcp_handle = sockets.add(dhcp_socket);

        let mut cmos = CMOS::new();

        let timeout = 30;
        let started = cmos.unix_time();

        println!("DHCP: {}", started);

        loop {
            if cmos.unix_time() - started > timeout {
                println!("ERROR: timeout");
                return;
            }

            let ms = (cmos.unix_time() * 1000000) as i64;
            let time = Instant::from_micros(ms);
            iface.poll(time, device, &mut sockets);
            let event = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll();

            println!("{:?}", event);

            match event {
                None => {}
                Some(Event::Configured(config)) => {
                    dhcp_config = Some((config.address, config.router, config.dns_servers));
                    println!("dhcp success");
                    break;
                }
                Some(Event::Deconfigured) => {}
            }

            if let Some(delay) = iface.poll_delay(time, &sockets) {
                let d = (delay.total_micros() as f64) / 10000.0;
                sleep(d.min(3.0)); // 0.1 seconds = 100 ms
            }
        }
    }

    if let Some((ip, gw, dns)) = dhcp_config {
        let dns: Vec<_> = dns.iter().map(|s| s.to_string()).collect();
        println!("NET DNS: {}", dns.join(", "));
        println!("NET IP: {}", ip);
        if let Some(gw) = gw {
            println!("NET GW: {}", gw);
        }
    } else {
        println!("dhcp failed");
    }
}
