use crate::driver::timer::cmos::CMOS;
use crate::driver::timer::pit::uptime;
use crate::println;
use crate::sys::net::socket::SOCKETS;
use crate::task::executor::sleep;
use alloc::string::ToString;
use core::str::FromStr;
use smoltcp::socket::dhcpv4;
use smoltcp::socket::dhcpv4::Event;
use smoltcp::time::Instant;
use smoltcp::wire::IpCidr;

pub fn main() {
    let mut dhcp_config = None;

    if let Some((ref mut iface, ref mut device)) = *crate::driver::nic::NET.lock() {
        let dhcp_socket = dhcpv4::Socket::new();
        let mut sockets = SOCKETS.lock();
        let dhcp_handle = sockets.add(dhcp_socket);

        let mut cmos = CMOS::new();

        let timeout = 30;
        let started = cmos.unix_time();

        loop {
            if cmos.unix_time() - started > timeout {
                println!("ERROR: timeout");
                return;
            }

            let ms = (cmos.unix_time() * 1000000) as i64;
            let time = Instant::from_micros(ms);
            iface.poll(time, device, &mut sockets);
            let event = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll();

            match event {
                None => {}
                Some(Event::Configured(config)) => {
                    iface.update_ip_addrs(|addrs| {
                        addrs.clear();
                        addrs
                            .push(IpCidr::from_str(config.address.to_string().as_str()).unwrap())
                            .unwrap();
                    });
                    if let Some(gw) = config.router {
                        if gw.to_string() == "0.0.0.0" {
                            iface.routes_mut().remove_default_ipv4_route();
                        } else {
                            iface.routes_mut().add_default_ipv4_route(gw).unwrap();
                        }
                    }
                    dhcp_config = Some((config.address, config.router, config.dns_servers));
                    break;
                }
                Some(Event::Deconfigured) => {}
            }

            if let Some(delay) = iface.poll_delay(time, &sockets) {
                let d = (delay.total_micros() as f64) / 10000.0;
                sleep(d.min(0.1)); // 0.1 seconds = 100 ms
            }
        }
    }

    if let Some((ip, _, _)) = dhcp_config {
        let uptime = uptime();
        println!(
            "\x1b[93m[{uptime:.6}]\x1b[0m DHCP Config Done! IP Address: {}",
            ip
        );
    } else {
        println!("dhcp failed");
    }
}
