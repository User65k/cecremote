use cec_linux::{
    CecDevice, CecLogAddrType, CecLogAddrs, CecLogicalAddress, CecModeFollower, CecModeInitiator,
    CecOpcode, CecPrimDevType, Version, CEC_VENDOR_ID_NONE, CecPowerStatus
};
use sispm::{get_devices, GlobalSiSPM};
use std::convert::TryInto;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{thread, time};
use std::convert::TryFrom;

mod monitor;
mod sock;
mod snapclient;

use monitor::mon;
use sock::{listen_for_vol_changes, setup_sock};

#[derive(Default)]
pub struct GState {
    /// TV is playing
    tv: Option<bool>,
    audio_mode: Option<bool>,
    avr_ready: bool,
    avr_standby: Option<bool>,
    cec_addr: Option<CecLogicalAddress>,
}

pub struct Actor {
    cec: CecDevice,
    pwr_socket: GlobalSiSPM,
}

fn main() -> std::io::Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let die_c = Arc::downgrade(&stop);
    if let Err(e) = ctrlc::set_handler(move || {
        if let Some(stop) = die_c.upgrade() {
            stop.swap(true, Ordering::Relaxed);
        }
    }) {
        match e {
            ctrlc::Error::NoSuchSignal(_) => unreachable!(),
            ctrlc::Error::MultipleHandlers => return Err(std::io::ErrorKind::AlreadyExists.into()),
            ctrlc::Error::System(e) => return Err(e),
        }
    }

    let listener = setup_sock();

    //send
    let cec_bus = CecDevice::open("/dev/cec0")?;
    let capas = cec_bus.get_capas()?;
    println!("capas  {:?}", capas);
    //cec_bus.set_mode(CecModeInitiator::Send, CecModeFollower::RepliesOnly)?;
    //monitor
    let cec_mon = CecDevice::open("/dev/cec0")?;
    cec_mon.set_mode(CecModeInitiator::None, CecModeFollower::Monitor)?;

    //clear address
    let log = CecLogAddrs::default();
    cec_bus.set_log(log)?;
    //set address
    let log = CecLogAddrs {
        cec_version: Version::V1_4,
        num_log_addrs: 1,
        vendor_id: CEC_VENDOR_ID_NONE,
        osd_name: "pi4".to_string().try_into().unwrap(),
        primary_device_type: [CecPrimDevType::PLAYBACK; 4],
        log_addr_type: [CecLogAddrType::PLAYBACK; 4],
        ..Default::default()
    };
    cec_bus.set_log(log)?;
    //cec_bus.set_phys(0x3300)?;

    let global_state = Arc::new(Mutex::new(GState::default()));
    let mutex = Arc::clone(&global_state);

    thread::spawn(move || mon(cec_mon, mutex));

    let pwr_socket = get_devices()
        .expect("on pwr socket")
        .pop()
        .expect("no pwr socket connected");

    let mut state = if pwr_socket.get_status(2).expect("status?") {
        //AVR has power...
        MediaState::AVRHasPwr
    } else {
        MediaState::Off
    };

    let actor = Arc::new(Mutex::new(Actor {
        cec: cec_bus,
        pwr_socket,
    }));
    let act = Arc::clone(&actor);
    thread::spawn(move || listen_for_vol_changes(listener, act));

    //monitor audio status
    let pw_plays = Arc::new(AtomicBool::new(false));
    let shared1 = Arc::downgrade(&pw_plays);
    //let (pw_sender, pw_receiver) = pipewire::channel::channel();
    //thread::spawn(move || pulse::watch(shared1, pw_receiver).expect("pw"));

    thread::spawn(move || snapclient::main(shared1).expect("pw err"));

    let cycle_time = time::Duration::from_millis(SLEEP_TIME_CYCLE_MS);
    let mut cycles_not_changed = 0;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(cycle_time);
        let GState {
            tv,
            audio_mode: _,
            avr_ready,
            avr_standby,
            cec_addr,
        } = *global_state.lock().unwrap();
        let pulse = false; //pw_plays.load(Ordering::Relaxed);
        
        state = match &state {
            MediaState::Watching if tv == Some(false) => {
                //TODO get_device_power_status to check AVR?
                println!("Watching: {tv:?} {pulse}");
                // TV turned Off
                let m = actor.lock().expect("main lock");
                switch_light(&m.pwr_socket, false);
                if pulse {
                    MediaState::Playing
                } else {
                    MediaState::SwitchOff
                }
            }
            MediaState::Playing if tv == Some(true) => {
                println!("Playing: {tv:?} {pulse}");
                // TV turned On
                //TODO restore vol
                let m = actor.lock().expect("main lock");
                if let Some(from) = cec_addr {
                    cec_audio_mode_off(&m.cec, from);
                }
                switch_light(&m.pwr_socket, true);
                MediaState::Watching
            }
            MediaState::Playing if !pulse => {
                println!("Playing: {tv:?} {pulse}");
                // Audio turned Off
                //TODO restore vol
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue,
                };
                let m = actor.lock().expect("main lock");
                cec_audio_mode_off(&m.cec, from);
                MediaState::SwitchOff
            }
            MediaState::Off if pulse || tv == Some(true) => {
                println!("Off: tv={tv:?} pulse={pulse}");
                // Turn On
                cycles_not_changed = 0;
                let m = actor.lock().expect("main lock");
                switch_avr(&m.pwr_socket, true, &global_state);
                MediaState::WaitForAudio
            }
            MediaState::WaitForAudio if avr_ready => {
                println!("WaitForAudio+avr_ready: {tv:?} {pulse}");
                // ARV is available

                //FIXME Some(false) true -> cec cmd: Audiosystem -> Unregistered   RoutingChange: CecDatapacket([48, 0, 51, 0])
                //but only once...
                let m = actor.lock().expect("main lock");
                if tv == Some(true) {
                    switch_light(&m.pwr_socket, true);
                    MediaState::Watching
                } else if pulse {
                    //TODO store volume
                    let from = match cec_addr {
                        Some(a) => a,
                        None => continue,
                    };
                    cec_audio_mode(&m.cec, from);
                    let _ = request_pwr_state(&m.cec, from);
                    MediaState::Playing
                } else {
                    println!("<3>TV and Audio off. No need for AVR anymore");
                    MediaState::SwitchOff
                }
            }
            MediaState::WaitForAudio if cycles_not_changed == THREE_SEC_IN_CYCLES => {
                //AVR wont turn on but has power
                println!("<4>WaitForAudio takes too long");
                cycles_not_changed += 1;
                let from = match cec_addr {
                    Some(a) => a,
                    None => {
                        println!("<4>no address to send from");
                        continue;
                    }
                };
                let m = actor.lock().expect("main lock");
                if tv == Some(true) {
                    print_err(
                        m.cec.turn_on(from, CecLogicalAddress::Audiosystem),
                        "PwrOn audio",
                    );
                } else if pulse {
                    cec_audio_mode(&m.cec, from);
                }
                MediaState::WaitForAudio
            }
            MediaState::Watching if avr_standby != Some(false) => {
                println!("<5>Watching: avr standby: {avr_standby:?}");
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue,
                };
                let m = actor.lock().expect("main lock");
                //TV is running but audio is off
                print_err(
                    m.cec.turn_on(from, CecLogicalAddress::Audiosystem),
                    "PwrOn audio",
                );
                let _ = request_pwr_state(&m.cec, from);
                MediaState::Watching
            }
            MediaState::Playing if avr_standby != Some(false) => {
                // none -> ask for standby status
                println!("<5>Playing: avr standby: {avr_standby:?}"); //None -> is not the reason the TV turns on
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue,
                };
                let m = actor.lock().expect("main lock");
                //Pulse is running but audio is off
                cec_audio_mode(&m.cec, from);
                let _ = request_pwr_state(&m.cec, from);
                MediaState::Playing
            }
            MediaState::AVRHasPwr => {
                println!("AVRHasPwr: {avr_standby:?}");
                match avr_standby {
                    None => {
                        // Service started, dont know whats up
                        let from = match cec_addr {
                            Some(a) => a,
                            None => continue,
                        };
                        let m = actor.lock().expect("main lock");
                        match request_pwr_state(&m.cec, from)
                        {
                            Some(CecPowerStatus::Standby) => MediaState::SwitchOff,
                            Some(CecPowerStatus::On) => MediaState::WaitForAudio,
                            _ => MediaState::AVRHasPwr,
                        }
                    }
                    Some(false) => {
                        //AVR is on
                        //TODO query Audio Address
                        MediaState::WaitForAudio
                    }
                    Some(true) => {
                        //AVR is in standby
                        MediaState::SwitchOff
                    }
                }
            }
            MediaState::SwitchOff => {
                let m = actor.lock().expect("main lock");
                if m.pwr_socket.get_status(2).expect("get avr pwr") {
                    println!("Off but AVR on. Standby: {avr_standby:?}");
                    if avr_standby == Some(true) {
                        // stay off
                        // this is enforcing a delay before cutting the AVR power
                        switch_avr(&m.pwr_socket, false, &global_state);
                        MediaState::Off
                    } else {
                        //send AVR to standby first
                        let from = match cec_addr {
                            Some(a) => a,
                            None => continue,
                        };
                        print_err(
                            m.cec.transmit(
                                from,
                                CecLogicalAddress::Audiosystem,
                                CecOpcode::Standby,
                            ),
                            "SendStandbyDevices audio",
                        );
                        let _ = request_pwr_state(&m.cec, from);
                        MediaState::SwitchOff
                    }
                }else{
                    //already off
                    MediaState::Off
                }
            }
            MediaState::Off => MediaState::Off, //stay forever
            MediaState::Watching => MediaState::Watching, //stay forever
            MediaState::Playing => MediaState::Playing,   //stay forever
            s => {
                if cycles_not_changed > FOUR_SEC_IN_CYCLES {
                    println!("<3>Hang in State {:?}", s);
                    MediaState::Off
                } else {
                    cycles_not_changed += 1;
                    *s
                }
            }
        };
    }
    //let _ = pw_sender.send(()); //end pw loop
    println!("Bye");
    Ok(())
}
const SLEEP_TIME_CYCLE_MS: u64 = 250;
const FOUR_SEC_IN_CYCLES: u8 = (4_000 / SLEEP_TIME_CYCLE_MS) as u8;
const THREE_SEC_IN_CYCLES: u8 = (3_000 / SLEEP_TIME_CYCLE_MS) as u8;


///request PWR state of Audiosystem and block till answered
#[inline]
fn request_pwr_state(cec: &CecDevice, from: CecLogicalAddress) -> Option<CecPowerStatus>
{
    cec
    .request_data(
        from,
        CecLogicalAddress::Audiosystem,
        CecOpcode::GiveDevicePowerStatus,
        b"",
        CecOpcode::ReportPowerStatus,
    )
    .ok()
    .and_then(|data| data.first().copied())
    .and_then(|s| CecPowerStatus::try_from(s).ok())    
}

#[inline]
fn switch_light(pwr_socket: &GlobalSiSPM, on: bool) {
    print_err(pwr_socket.set_status(1, on), "pwr1");
}
#[inline]
fn switch_avr(pwr_socket: &GlobalSiSPM, on: bool, state: &Arc<Mutex<GState>>) {
    let mut s = state.lock().unwrap();
    s.avr_ready = false;
    s.avr_standby = None;
    s.audio_mode = None;
    print_err(pwr_socket.set_status(2, on), "pwr2");
}
#[derive(Copy, Clone, Debug)]
enum MediaState {
    /// TV has audio
    ///
    /// Everything is on
    Watching,
    /// snapcast has audio
    ///
    /// Only AVR is on
    Playing,
    /// power down AVR socket
    SwitchOff,
    /// just do nothing
    Off,
    /// AVR has power, but not booted yet.
    ///
    /// Everything else is powered off.
    ///
    /// Ends once Audio sends ReportPhysicalAddr
    WaitForAudio,
    /// Initial State - Audio has power, standby is unknown
    AVRHasPwr,
}

/// requests audio focus. Turn on AVR if needed
fn cec_audio_mode(cec: &CecDevice, from: CecLogicalAddress) {
    /*
        #The feature can be initiated from a device (eg TV or STB) or the amplifier. In the case of initiation by a device
        #other than the amplifier, that device sends an <System Audio Mode Request> to the amplifier, with the
        #physical address of the device that it wants to use as a source as an operand. Note that the Physical Address
        #may be the TV or STB itself.

    The amplifier comes out of standby (if necessary) and switches to the relevant connector for device specified by [Physical Address].
    It then sends a <Set System Audio Mode> [On] message.

    ...  the device requesting this information can send the volume-related <User Control Pressed> or <User Control Released> messages.
    */
    print_err(
        cec.transmit_data(
            from,
            CecLogicalAddress::Audiosystem,
            CecOpcode::SystemAudioModeRequest,
            b"\x33\x00",
        ),
        "SystemAudioModeRequest failed",
    );
    //print_err(cec.audio_get_status(),"GiveAudioStatus");
}
///Print an error
fn print_err<E: std::fmt::Debug>(res: Result<(), E>, name: &str) {
    if let Err(e) = res {
        println!("<3>{} Err: {:?}", name, e);
    }
}
///requests termination of audio focus
fn cec_audio_mode_off(cec: &CecDevice, from: CecLogicalAddress) {
    /*
    <System Audio Mode Request> sent without a [Physical Address] parameter requests termination of the feature.
    In this case, the amplifier sends a <Set System Audio Mode> [Off] message.
        */
    print_err(
        cec.transmit(
            from,
            CecLogicalAddress::Audiosystem,
            CecOpcode::SystemAudioModeRequest,
        ),
        "SystemAudioModeRequest off",
    );
}
