use crate::{print_err, Actor};
use cec_linux::CecLogicalAddress;
use std::env;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

pub fn setup_sock() -> UnixListener {
    let pid = env::var("LISTEN_PID");
    let fds = env::var("LISTEN_FDS");
    //let env = env::vars();
    //println!("env {:?} {:?}", pid, fds);
    if pid
        .ok()
        .and_then(|x| x.parse::<u32>().ok())
        .is_some_and(|x| x == std::process::id())
        && fds
            .ok()
            .and_then(|x| x.parse::<usize>().ok())
            .is_some_and(|x| x == 1)
    {
        unsafe { UnixListener::from_raw_fd(3) }
    } else {
        println!("no FD");
        UnixListener::bind("/tmp/cec").expect("faild to listen on UDS")
    }
}
pub fn listen_for_vol_changes(listener: UnixListener, act: Arc<Mutex<Actor>>) {
    let mut buf = [0u8; 1];
    for mut stream in listener.incoming().flatten() {
        //if let Ok(mut stream) = stream {
        if let Ok(()) = stream.read_exact(&mut buf) {
            //0-100 Vol
            //&0x80 on/off
            //&0xC0 activesource
            let n = buf[0];
            match n {
                1..=100 => {
                    set_volume(&act, n);
                }
                0 => println!("mute"),
                101..=127 => println!("?"),
                0x80..=u8::MAX => match n & 0xF8 {
                    0x80 => {
                        //request sispm to be switched
                        let on = n & 0x04 != 0;
                        let mut n = n & 0x03;
                        if n == 0 {
                            n = 4;
                        }
                        println!("switch {} {}", n, on);
                        act.lock()
                            .expect("could not lock for ctrl sock")
                            .pwr_socket
                            .set_status(n, on)
                            .expect("pwr boom");
                    }
                    0xC0 => {
                        //request active source to be 3.x.0.0
                        //                              5 = SteamDeck Game
                        //                              3 = Pi Cable/Sat
                        //                              ? = Ps4
                        //4 blueRay, 1 DVD/BlueRay, 2 Media Player
                        let data = [0x30 + (n & 7), 0];

                        let act = act.lock().expect("could not lock for ctrl sock");

                        let from = match act.cec.get_log()
                        .ok().and_then(|l|l.addresses().first().copied()).unwrap_or(CecLogicalAddress::UnregisteredBroadcast) {
                            CecLogicalAddress::UnregisteredBroadcast => continue,
                            a => a
                        };

                        print_err(
                            act.cec.transmit_data(
                                from,
                                CecLogicalAddress::UnregisteredBroadcast,
                                cec_linux::CecOpcode::ActiveSource,
                                &data,
                            ),
                            "cec boom SetStreamPath",
                        );
                    }
                    _ => println!("?"),
                },
            }
        }
        //}
    }
}

fn set_volume(act: &Arc<Mutex<Actor>>, vol: u8) {
    println!("Vol Requested: {}", vol);
    let cec = &act.lock().expect("could not lock for ctrl sock").cec;

    let from = match cec.get_log()
    .ok().and_then(|l|l.addresses().first().copied()).unwrap_or(CecLogicalAddress::UnregisteredBroadcast) {
        CecLogicalAddress::UnregisteredBroadcast => return,
        a => a
    };

    super::set_volume(cec, from, vol, None);
}
