
use std::cell::RefCell;
use std::collections::HashSet;
use std::net::{IpAddr, UdpSocket};
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::JoinHandle;
use std::time::Duration;

use getifaddrs::getifaddrs;
use slint::{ModelRc, Timer, TimerMode, VecModel};

slint::include_modules!();

struct ListenerThread {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

struct ListenerState {
    listen_enabled: bool,
    selected_ip: Option<String>,
    listener: Option<ListenerThread>,
}

fn stop_listener(state: &mut ListenerState) {
    if let Some(listener) = state.listener.take() {
        listener.stop.store(true, Ordering::Relaxed);
        let _ = listener.handle.join();
    }
}

fn restart_listener(state: &mut ListenerState, app_window_weak: &slint::Weak<AppWindow>) {
    stop_listener(state);

    if !state.listen_enabled {
        return;
    }

    let Some(ip) = state.selected_ip.clone() else {
        eprintln!("No IPv4 address available for selected interface");
        return;
    };

    let stop = Arc::new(AtomicBool::new(false));
    let weak_window = app_window_weak.clone();
    let stop_for_thread = stop.clone();
    let handle = std::thread::spawn(move || {
        if let Err(e) = listen(&ip, weak_window, stop_for_thread) {
            eprintln!("Listener thread error: {e:?}");
        }
    });

    state.listener = Some(ListenerThread { stop, handle });
}

fn get_interface_ips() -> Vec<String> {
    let mut ips = Vec::new();
    let mut seen_ips = HashSet::new();

    if let Ok(addrs) = getifaddrs() {
        for iface in addrs {
            let Some(ip) = iface.address.ip_addr() else {
                continue;
            };
            let IpAddr::V4(v4) = ip else {
                continue;
            };

            let ip_string = v4.to_string();
            if seen_ips.insert(ip_string.clone()) {
                ips.push(ip_string);
            }
        }
    }

    ips
}

fn decode_artnet_packet(buf: &[u8]) -> Option<(u8, u8, u8, u8, u8)> {
    //Check if artnet timecode packet and return hour min sec frame and framerate
    if buf.len() < 12 {
        return None;
    }
    if &buf[0..8] != b"Art-Net\0" {
        return None;
    }
    if buf[8] != 0x50 || buf[9] != 0x00 {
        return None;
    }
    let hour = buf[10];
    let min = buf[11];
    let sec = buf[12];
    let frame = buf[13];
    let framerate = buf[14];
    Some((hour, min, sec, frame, framerate))
}

fn listen(
    ip: &str,
    app_window_weak: slint::Weak<AppWindow>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<()> {
    let bind_addr = format!("{}:{}", ip, 6454);
    let socket = UdpSocket::bind(&bind_addr)?;
    socket.set_nonblocking(true)?;
    while !stop.load(Ordering::Relaxed) {
        let mut buf = [0u8; 1024];
        match socket.recv_from(&mut buf) {
            Ok((size, _src)) => {
                let tc = decode_artnet_packet(&buf[..size]);
                if let Some((hour, min, sec, frame, framerate)) = tc {
                    let app_window_weak = app_window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app_window) = app_window_weak.upgrade() {
                            app_window.set_timecode(format!("{:02}:{:02}:{:02}:{:02}", hour, min, sec, frame).into());
                            app_window.set_fr(format!("{}", framerate).into());
                        }
                    });
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                eprintln!("Error receiving data: {:?}", e);
                break;
            }
        }
    }
    Ok(())
}

fn main() {
    let main_window = AppWindow::new().expect("failed to create main window");
    let initial_ips = get_interface_ips();
    let interface_ips = Rc::new(RefCell::new(initial_ips.clone()));
    let interface_model = Rc::new(VecModel::from(
        initial_ips
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>(),
    ));
    let model_rc = ModelRc::new(interface_model.clone());
    main_window.set_interfaces(model_rc);

    let listener_state = Rc::new(RefCell::new(ListenerState {
        listen_enabled: false,
        selected_ip: interface_ips.borrow().first().cloned(),
        listener: None,
    }));

    let timer = Timer::default();
    let interval = Duration::from_millis(1000);
    let callback = {
        let interface_model = interface_model.clone();
        let interface_ips = interface_ips.clone();
        let listener_state = listener_state.clone();
        let weak_window = main_window.as_weak();

        move || {
            let ips = get_interface_ips();

            interface_model.set_vec(
                ips.iter()
                    .cloned()
                    .map(slint::SharedString::from)
                    .collect::<Vec<_>>(),
            );
            *interface_ips.borrow_mut() = ips;

            let mut state = listener_state.borrow_mut();
            let current_ip_still_exists = state
                .selected_ip
                .as_ref()
                .is_some_and(|selected| interface_ips.borrow().iter().any(|ip| ip == selected));

            if !current_ip_still_exists {
                state.selected_ip = interface_ips.borrow().first().cloned();
                if state.listen_enabled {
                    restart_listener(&mut state, &weak_window);
                }
            }
        }
    };
    timer.start(TimerMode::Repeated, interval, callback);

    let weak_window = main_window.as_weak();

    main_window.on_set_interface({
        let listener_state = listener_state.clone();
        let interface_ips = interface_ips.clone();
        let weak_window = weak_window.clone();

        move |value| {
            let idx = value as usize;
            let selected_ip = interface_ips.borrow().get(idx).cloned();

            let mut state = listener_state.borrow_mut();
            state.selected_ip = selected_ip;
            if state.listen_enabled {
                restart_listener(&mut state, &weak_window);
            }
        }
    });
    main_window.on_set_listen({
        let listener_state = listener_state.clone();
        let weak_window = weak_window.clone();

        move |value| {
            let mut state = listener_state.borrow_mut();
            state.listen_enabled = value;
            if value {
                restart_listener(&mut state, &weak_window);
            } else {
                stop_listener(&mut state);
            }
        }
    });

    let run_result = main_window.run();
    {
        let mut state = listener_state.borrow_mut();
        stop_listener(&mut state);
    }

    match run_result {
        Ok(_) => (),
        Err(e) => eprintln!("Error running main window: {:?}", e),
    }
}
