use crate::{Actor, print_err};
use cec_linux::{CecLogicalAddress, CecOpcode, CecUserControlCode};
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
            match buf[0] {
                1..=100 => {
                    set_volume(&act, buf[0]);
                }
                0 => println!("mute"),
                n => match n & 0xF8 {
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
                        print_err(
                            act.lock()
                            .expect("could not lock for ctrl sock")
                            .cec
                            .transmit_data(
                                CecLogicalAddress::Playback2,
                                CecLogicalAddress::UnregisteredBroadcast,
                                cec_linux::CecOpcode::ActiveSource,
                                &data,
                            ),
                            "cec boom SetStreamPath");
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
    if let Some(v) = cec.request_data(
        CecLogicalAddress::Playback2,
        CecLogicalAddress::Audiosystem,
        CecOpcode::GiveAudioStatus,
        b"",
        CecOpcode::ReportAudioStatus,
    ).ok().and_then(|d|d.first().copied()) {
        println!("Vol is: Muted: {} Vol: {}%", v & 0x80, v & 0x7f);
        let steps = vol as i8 - (v & 0x7f) as i8;
        let key = if steps.is_positive() {
            CecUserControlCode::VolumeUp
        } else {
            CecUserControlCode::VolumeDown
        };
        let steps = steps.unsigned_abs() * 2;
        for _ in 0..steps {
            if let Err(e) = cec.keypress(
                CecLogicalAddress::Playback2,
                CecLogicalAddress::Audiosystem,
                key,
            ) {
                println!("<3>keypress Err: {:?}", e);
                return;
            }
        }
    }
}