use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::io::{Read, Write};
use std::convert::TryInto;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Weak;
use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex};
use crate::Actor;

pub fn main(
    playing: Weak<AtomicBool>,
    act: Arc<Mutex<Actor>>,
    snapclient_vol: Arc<Mutex<u8>>,
) -> Result<(), std::io::Error> {

    let listener = TcpListener::bind("127.0.0.1:0")?;

    loop {
        act.lock().unwrap().pwr_socket.set_status(2, true).unwrap();

        let mut snapclient = Command::new("snapclient")
        .args(["-h", "127.0.0.1", "-p", &format!("{}", listener.local_addr()?.port()), "--logsink", "system", "-s", "14", "--mixer", "none"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
        //journalctl -t snapclient
        loop {
            let (client, _ ) = listener.accept()?;
            if let Err(e) = fwd(
                client,
                &playing,
                &snapclient_vol
            ) {
                println!("<3>snapcast error: {}", e);
                break;
            }
            match snapclient.try_wait()? {
                None => {
                    //still running
                    continue;
                },
                Some(_) => {
                    //process ended
                    break;
                },
            }
        };
        
        let status = snapclient.wait()?;
        println!("<5>snapclient exit: {}", status);
        playing.upgrade().unwrap().store(false, Ordering::Relaxed);
    }
}

fn fwd(
    client: TcpStream,
    playing: &Weak<AtomicBool>,
    snapclient_vol: &Arc<Mutex<u8>>,
) -> Result<(), std::io::Error> {
    let server = TcpStream::connect("127.0.0.1:1704")?;

    let mut s = server.try_clone()?;
    let mut c = client.try_clone()?;

    let playing = playing.clone();
    let snapclient_vol = snapclient_vol.clone();

    thread::spawn(move || {
        if let Err(e) = server_to_client(server, client, &playing, snapclient_vol) {
            println!("<3>s2c err: {}", e);
        }
        playing.upgrade().unwrap().store(false, Ordering::Relaxed);
    });

    let mut buffer = [0u8;1024*17];
    //forward client -> server
    while let Ok(r) = c.read(&mut buffer) {
        if r==0 {
            println!("<5>c2s done");
            return Ok(());
        }
        if let Err(e) = s.write_all(&buffer[..r]) {
            println!("<3>c2s error: {}", e);
            return Err(e);
        }
    }
    Ok(())
}

fn server_to_client(
    mut server: TcpStream,
    mut client: TcpStream,
    playing: &Weak<AtomicBool>,
    snapclient_vol: Arc<Mutex<u8>>,
) -> Result<(), std::io::Error> {
    let mut last = Instant::now();
    let mut playing_now = false;
    loop {
        let mut buf = [0u8;1024*17];
        //forward client -> server
        server.read_exact(&mut buf[..26])?;

        let typ = u16::from_le_bytes(buf[..2].try_into().unwrap());
        let size = u32::from_le_bytes(buf[22..26].try_into().unwrap()) as usize;

        if size+26 > buf.len() {
            panic!("huge packet {}", size+26);
        }

        server.read_exact(&mut buf[26..size+26])?;

        match typ {
            3 => {
                // ServerSettings
                // 4b size + {"bufferMs":1000,"latency":0,"muted":false,"volume":100}
                let json = &buf[26+4..size+26];
                let m = get_json_val(&buf, b"muted").is_some_and(|v|v==b"true");
                let v = get_json_val(&buf, b"volume").map(|s|{
                    let vol: u32 = s.iter()
                     .rev().enumerate().map(|(i,&v)| 10u32.pow(i as u32)*(v-b'0') as u32).sum();
                    vol
                });

                if let Some(vol) = v {
                    /*
                    keep ears intact
                    100 -> 80
                    1 -> 20
                     */
                    *snapclient_vol.lock().unwrap() = ((vol+34) as f32  *0.6) as u8;
                }

                println!("ServerSettings: {} m:{} v:{:?}", std::str::from_utf8(json).unwrap(), m, v);
            }
            2 => {
                // Wire Chunk
                last = Instant::now();
                if !playing_now {
                    playing_now = true;
                    playing.upgrade().unwrap().store(true, Ordering::Relaxed);
                    println!("snapclient has data");
                }
            }
            4 if playing_now => {
                // Time
                //happens a lot, even if not playing
                let time_diff = Instant::elapsed(&last);
                if time_diff > Duration::new(5, 0) {
                    playing_now = false;
                    playing.upgrade().unwrap().store(false, Ordering::Relaxed);
                    println!("snapclient no data since 5s");
                }
            }
            1 => { //Codec Header
            }
            _ => {
                //println!("data {} {:?}", typ, buf[..size+26]);
            }
        }
        


        client.write_all(&buf[..size+26])?;
    }
}


/// search a JSON value inside of a byte stream.
fn get_json_val<'a>(haystack: &'a [u8], param: &'_ [u8]) -> Option<&'a [u8]> {
    if let Some(pos) = haystack.windows(param.len() + 3).position(|window| {
        &window[1..param.len() + 1] == param
            && window[0] == b'"'
            && &window[param.len() + 1..] == b"\":"
    }) {
        let start = pos + param.len() + 3;
        if let Some(len) = haystack[start..]
            .iter()
            .position(|&window| window == b',' || window == b'}')
        {
            return Some(&haystack[start..start + len]);
        }
    }
    None
}