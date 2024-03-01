use std::process::{Command, Stdio};
use std::thread;
use std::io::Read;
use std::convert::TryInto;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Weak;
use std::rc::Rc;
use std::cell::{RefCell, RefMut};

// Copyright The pipewire-rs Contributors.
// SPDX-License-Identifier: MIT
//
// This file is a rustic interpretation of the the [PipeWire Tutorial 4][tut]
//
// tut: https://docs.pipewire.org/page_tutorial4.html

use pipewire as pw;
use pw::{properties, spa, stream::{Stream, StreamFlags}};
use spa::pod;


pub const DEFAULT_RATE: u32 = 44100;
pub const DEFAULT_CHANNELS: u32 = 2;
pub const CHAN_SIZE: usize = std::mem::size_of::<i16>();

pub fn main(
    playing: Weak<AtomicBool>,
) -> Result<(), pw::Error> {

    let snapclient = Command::new("snapclient")
    .args(["-h", "127.0.0.1", "--logsink", "stderr", "--player", "file", "--mixer", "script:/home/pi/cec_vol.py"])
    .stdin(Stdio::null())
    .stdout(Stdio::piped()) //music S16LE
    .stderr(Stdio::piped()) //logs
    .spawn()
    .expect("ls command failed to start");

    let sound_in_stream = Rc::new(RefCell::new(snapclient.stdout.unwrap()));
    let mut snap_logs = snapclient.stderr.unwrap();

    thread::spawn(move || {
        let mut buffer = [0u8;255];
        let mut pos = 0;
        let mut no_chunks = false;
        while let Ok(n) = snap_logs.read(&mut buffer[pos..]) {
            if n==0 {
                println!("snapclient log done");
                break;
            }
            pos += n;
            if buffer[pos-1] == b'\n' || pos == buffer.len() {
                if let Ok(s) = std::str::from_utf8(&buffer[..pos-1]) {
                    if s.ends_with("No chunks available") {
                        if !no_chunks {
                            no_chunks = true;
                            println!("snapclient: No chunks available");
                        }
                    }else{
                        //2024-01-22 14-55-27.104
                        println!("<7>snapclient log: {}", &s[24..]);
                        no_chunks = false;
                    }
                }
                pos = 0;
            }
        }
    });
    pw::init();
    let mut buffer = [0u8;8820];//0.05s
    loop {
        let sound_in_stream = sound_in_stream.clone();
        playing
            .upgrade()
            .unwrap()
            .store(false, Ordering::Relaxed);
        
        while let Ok(n) = sound_in_stream.borrow_mut().read(&mut buffer[..]) {
            if n==0 {
                println!("snapclient sound done");
                return Ok(());
            }
            if n>=8 && u64::from_ne_bytes(buffer[..8].try_into().unwrap())==0 {
                //no sound
            }else{
                break;
            }
        }
        println!("snapclient plays");
        playing
            .upgrade()
            .unwrap()
            .store(true, Ordering::Relaxed);

        let mainloop = pw::MainLoop::new()?;
        let context = pw::Context::new(&mainloop)?;
        let core = context.connect(None)?;

        let stream = Stream::new(
            &core,
            "audio-src",
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Music",
                *pw::keys::MEDIA_CATEGORY => "Playback",
            },
        )?;

        let _listener = stream
            .add_local_listener_with_user_data(mainloop.clone())
            .process(move |stream, main_loop| match stream.dequeue_buffer() {
                None => println!("No buffer received"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    let stride = CHAN_SIZE * DEFAULT_CHANNELS as usize;
                    let data = &mut datas[0];
                    let n_frames = if let Some(slice) = data.data() {
                        /*let n_frames = slice.len() / stride;
                        for i in 0..n_frames {
                            *acc += PI_2 * 440.0 / DEFAULT_RATE as f64;
                            if *acc >= PI_2 {
                                *acc -= PI_2
                            }
                            let val = (f64::sin(*acc) * DEFAULT_VOLUME * 16767.0) as i16;
                            for c in 0..DEFAULT_CHANNELS {
                                let start = i * stride + (c as usize * CHAN_SIZE);
                                let end = start + CHAN_SIZE;
                                let chan = &mut slice[start..end];
                                chan.copy_from_slice(&i16::to_le_bytes(val));
                            }
                        }
                        n_frames*/
                        let b = sound_in_stream.borrow_mut().read(slice).expect("read");
                        if b>=8 && slice.len() >= 8 && u64::from_ne_bytes(slice[..8].try_into().unwrap())==0 {
                            //no sound
                            println!("snapclient read 0s");
                            let _ = stream.disconnect();
                            main_loop.quit();
                            0
                        }else{
                            println!("snapclient data {}", b);
                            b / stride
                        }
                    } else {
                        println!("snapclient no data buffer");
                        0
                    };
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = stride as _;
                    *chunk.size_mut() = (stride * n_frames) as _;
                }
            })
            .register()?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(spa::param::audio::AudioFormat::S16LE);
        audio_info.set_rate(DEFAULT_RATE);
        audio_info.set_channels(DEFAULT_CHANNELS);

        let values: Vec<u8> = pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pod::Value::Object(pod::Object {
                type_: spa::sys::SPA_TYPE_OBJECT_Format,
                id: spa::sys::SPA_PARAM_EnumFormat,
                properties: audio_info.into(),
            }),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [pod::Pod::from_bytes(&values).unwrap()];

        stream.connect(
            spa::Direction::Output,
            None,
            StreamFlags::AUTOCONNECT
                | StreamFlags::MAP_BUFFERS
                | StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        println!("snapclient pw loop");
        mainloop.run();
        println!("snapclient paused");
    }

    Ok(())
}
