use cec_linux::{CecDevice, CecModeFollower, CecModeInitiator, CecLogAddrs, OSDStr, CecPrimDevType, CecLogAddrType, Version, CEC_VENDOR_ID_NONE, CecMsg, CecOpcode, CecLogicalAddress, CecEvent, CecLogAddrMask, PollFlags};
use libpulse_binding::{context::introspect::SinkInputInfo, callbacks::ListResult};
use std::{thread, time,boxed::Box};
use std::convert::TryInto;
use sispm::{get_devices, GlobalSiSPM};
use std::time::Duration;
use std::sync::{Arc, Mutex};
use std::os::unix::net::UnixListener;
use std::io::Read;
use std::env;
use std::os::fd::FromRawFd;

mod pulse;
///Strg+C was received
static mut DIE: bool = false;
static mut PULSE_PLAYS: bool = false;

#[derive(Default)]
struct GState {
  /// TV is playing
  tv: Option<bool>,
  audio_mode: Option<bool>,
  avr_ready: bool,
  avr_standby: Option<bool>,
  cec_addr: Option<CecLogicalAddress>
}

fn listen_for_vol_changes(listener: UnixListener) {
  let mut buf = [0u8;1];
  for mut stream in listener.incoming().flatten() {
    //if let Ok(mut stream) = stream {
      if let Ok(()) = stream.read_exact(&mut buf) {
        //0-100 Vol
        //&0x80 on/off
        match buf[0] {
          1..=100 => {
          println!("Vol Requested: {}", buf[0]);
          },
          0 => println!("mute"),
          n if n&0xF8 == 0x80 => {
            //request sispm to be switched
            let on = n&0x04!=0;
            let n = n & 0x03;
            println!("switch {} {}",n, on);
          },
          _ => println!("?"),
        }
      }
    //}
  }
}

fn main() -> std::io::Result<()> {
    if let Err(e) = ctrlc::set_handler(move || {
        unsafe {DIE = true;}
    }) {
        match e {
            ctrlc::Error::NoSuchSignal(_) => unreachable!(),
            ctrlc::Error::MultipleHandlers => return Err(std::io::ErrorKind::AlreadyExists.into()),
            ctrlc::Error::System(e) => return Err(e),
        }
    }
    
    let pid = env::var("LISTEN_PID");
    let fds = env::var("LISTEN_FDS");
    //let env = env::vars();
    //println!("env {:?} {:?}", pid, fds);
    let listener = if pid.ok().and_then(|x|x.parse::<u32>().ok()).is_some_and(|x| x == std::process::id())
    && fds.ok().and_then(|x|x.parse::<usize>().ok()).is_some_and(|x|x==1) {
        unsafe{UnixListener::from_raw_fd(3)}
    }else{
      println!("no FD");
      UnixListener::bind("/tmp/cec_vol").expect("faild to listen on UDS")
    };
    thread::spawn(move || listen_for_vol_changes(listener));
    
    let global_state = Arc::new(Mutex::new(GState::default()));
    let mut mutex = Arc::clone(&global_state);

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
    
    //.base_device(CecLogicalAddress::Audiosystem).hdmi_port(3)
    //.physical_address(0x3300) //3.3.0.0
    thread::spawn(move || loop {
        let f = cec_mon.poll(
            PollFlags::POLLIN | PollFlags::POLLRDNORM | PollFlags::POLLPRI,
            -1,
        ).unwrap();
        if f.intersects(PollFlags::POLLPRI) {
            if let CecEvent::StateChange(s) = cec_mon.get_event().unwrap() {
                println!("<7>{:?}", s);
                if s.log_addr_mask.contains(CecLogAddrMask::Playback1) {
                    mutex.lock().unwrap().cec_addr = Some(CecLogicalAddress::Playback1);
                }else if s.log_addr_mask.contains(CecLogAddrMask::Playback2) {
                    mutex.lock().unwrap().cec_addr = Some(CecLogicalAddress::Playback2);
                }else if s.log_addr_mask.contains(CecLogAddrMask::Playback3) {
                    mutex.lock().unwrap().cec_addr = Some(CecLogicalAddress::Playback3);
                }else{
                    mutex.lock().unwrap().cec_addr = None;
                }
            }
        }
        if f.contains(PollFlags::POLLIN | PollFlags::POLLRDNORM) {
            let msg = cec_mon.rec().unwrap();
            command(msg, &mut mutex)
        }
    });

    let pwr_socket = get_devices().expect("on pwr socket")
        .pop().expect("no pwr socket connected");

    let mut state = if pwr_socket.get_status(2).expect("status?") {
        //AVR has power...
        MediaState::AVRHasPwr
    }else{
        MediaState::Off
    };

    //monitor audio status
    let _a = pulse::NotWhenAudio::new(|ctx| {
        ctx.introspect()
            .get_sink_input_info_list(pulse_callback);
    }).expect("pulse watcher");

    let cycle_time = time::Duration::from_millis(SLEEP_TIME_CYCLE_MS);
    let mut cycles_not_changed = 0;
    while !(unsafe {DIE}) {
        let GState {
    tv,
    audio_mode: _,
    avr_ready,
    avr_standby,
    cec_addr,    
  } = *global_state.lock().unwrap();
  let pulse = unsafe { PULSE_PLAYS };
    /*if avr_standby == Some(false) && target_vol != cec_vol {
      
    }*/
        state = match &state {
            MediaState::Watching if tv == Some(false) => {//TODO get_device_power_status to check AVR?
                println!("Watching: {tv:?} {pulse}");
                // TV turned Off
                switch_light(&pwr_socket, false);
                switch_subwoofer(&pwr_socket, false);
                if pulse {
                    MediaState::Playing
                }else{
                    MediaState::Off
                }
            },
            MediaState::Playing if tv == Some(true) => {
                println!("Playing: {tv:?} {pulse}");
                // TV turned On
//TODO restore vol
                if let Some(from) = cec_addr {
                    cec_audio_mode_off(&cec_bus, from);
                }
                switch_light(&pwr_socket, true);
                MediaState::Watching
            },
            MediaState::Playing if !pulse => {
                println!("Playing: {tv:?} {pulse}");
                // Audio turned Off
//TODO restore vol
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue
                };
                cec_audio_mode_off(&cec_bus, from);
                switch_subwoofer(&pwr_socket, false);
                MediaState::Off
            },
            MediaState::Off if pulse || tv == Some(true) => {
                println!("Off: tv={tv:?} pulse={pulse}");
                // Turn On
                cycles_not_changed = 0;
                switch_avr(&pwr_socket, true, &global_state);
                MediaState::WaitForAudio
            },
            MediaState::WaitForAudio if avr_ready => {
                println!("WaitForAudio+avr_ready: {tv:?} {pulse}");
                // ARV is available
                
                //FIXME Some(false) true -> cec cmd: Audiosystem -> Unregistered   RoutingChange: CecDatapacket([48, 0, 51, 0])
                //but only once...
                switch_subwoofer(&pwr_socket, true);
                if tv == Some(true) {
                    switch_light(&pwr_socket, true);
                    MediaState::Watching
                }else if pulse {
                    //TODO store volume
                    let from = match cec_addr {
                        Some(a) => a,
                        None => continue
                    };
                    cec_audio_mode(&cec_bus, from);
                    print_err(cec_bus.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::GiveDevicePowerStatus),"Request AVR stadby state");
                    MediaState::Playing
                }else{
                    println!("<3>TV and Audio off. No need for AVR anymore");
                    MediaState::Off
                }
            },
            MediaState::WaitForAudio if cycles_not_changed == THREE_SEC_IN_CYCLES => {
                //AVR wont turn on but has power
                println!("<4>WaitForAudio takes too long");
                cycles_not_changed += 1;
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue
                };
                if tv == Some(true) {
                    print_err(cec_bus.turn_on(from, CecLogicalAddress::Audiosystem),"PwrOn audio");
                }else if pulse {
                    cec_audio_mode(&cec_bus, from);
                }
                MediaState::WaitForAudio
            },
            MediaState::Watching if avr_standby != Some(false) => {
                println!("<5>Watching: avr standby: {avr_standby:?}");
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue
                };
                //TV is running but audio is off
                print_err(cec_bus.turn_on(from, CecLogicalAddress::Audiosystem),"PwrOn audio");
                //TODO request PWR state?
                print_err(cec_bus.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::GiveDevicePowerStatus),"Request AVR stadby state");
                MediaState::Watching
            },
            MediaState::Playing if avr_standby != Some(false) => {
            // none -> ask for standby status
                println!("<5>Playing: avr standby: {avr_standby:?}"); //None -> is not the reason the TV turns on
                let from = match cec_addr {
                    Some(a) => a,
                    None => continue
                };
                //Pulse is running but audio is off
                cec_audio_mode(&cec_bus, from);
                print_err(cec_bus.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::GiveDevicePowerStatus),"Request AVR stadby state");
                MediaState::Playing
            },
            MediaState::AVRHasPwr => {
                println!("AVRHasPwr: {avr_standby:?}");
                match avr_standby {
                  None => {
                    // Service started, dont know whats up
                    if let Some(from) = cec_addr {
                        print_err(cec_bus.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::GiveDevicePowerStatus),"Request AVR stadby state");
                    }
                    MediaState::AVRHasPwr
                  },
                  Some(false) => {
                    //AVR is on
                    //TODO query Audio Address
                    MediaState::WaitForAudio
                  },
                  Some(true) => {
                    //AVR is in standby
                    MediaState::Off
                  }
                }
            },
            MediaState::Off => {
                if pwr_socket.get_status(2).expect("get avr pwr") {
                    println!("Off but AVR on. Standby: {avr_standby:?}");
                    if avr_standby==Some(true) {
                        // stay off
                        // this is enforcing a delay before cutting the AVR power
                        switch_avr(&pwr_socket, false, &global_state);
                    }else{
                        //send AVR to standby first
                        let from = match cec_addr {
                            Some(a) => a,
                            None => continue
                        };
                        print_err(cec_bus.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::Standby),"SendStandbyDevices audio");
                        print_err(cec_bus.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::GiveDevicePowerStatus),"Request AVR stadby state");
                    }
                }
                MediaState::Off
            },
            MediaState::Watching => MediaState::Watching,//stay forever
            MediaState::Playing => MediaState::Playing,//stay forever
            s => {
                if cycles_not_changed > FOUR_SEC_IN_CYCLES {
                    println!("<3>Hang in State {:?}", s);
                    MediaState::Off
                }else{
                    cycles_not_changed += 1;
                    *s
                }
            }
        };
        thread::sleep(cycle_time);
    }
    println!("Bye");
    Ok(())
}
const SLEEP_TIME_CYCLE_MS: u64 = 250;
const FOUR_SEC_IN_CYCLES: u8 = (4_000/SLEEP_TIME_CYCLE_MS) as u8;
const THREE_SEC_IN_CYCLES: u8 = (3_000/SLEEP_TIME_CYCLE_MS) as u8;

#[inline]
fn switch_light(pwr_socket: &GlobalSiSPM, on: bool) {
    print_err(pwr_socket.set_status(1, on),"pwr1");
}
#[inline]
fn switch_avr(pwr_socket: &GlobalSiSPM, on: bool, state: &Arc<Mutex<GState>>) {
  let mut s = state.lock().unwrap();
  s.avr_ready = false;
  s.avr_standby = None;
  s.audio_mode = None;
    print_err(pwr_socket.set_status(2, on),"pwr2");
}
#[inline]
fn switch_subwoofer(pwr_socket: &GlobalSiSPM, on: bool) {
    print_err(pwr_socket.set_status(3, on),"pwr3");
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
    Off,
    /// AVR has power, but not booted yet.
    /// 
    /// Everything else is powered off
    WaitForAudio,
    /// Initial State - Audio has power, standby is unknown
    AVRHasPwr
}


fn command(cmd: CecMsg, state: &mut Arc<Mutex<GState>>) {
    let opcode = match cmd.opcode() {
        Some(Ok(opc)) => opc,
        _ => return,
    };
    match opcode {
        CecOpcode::Standby if cmd.initiator() == CecLogicalAddress::Tv => {
          state.lock().unwrap().tv = Some(false);
            println!("======== Tv aus ===========")
        },
        CecOpcode::ActiveSource if cmd.initiator() == CecLogicalAddress::Tv && cmd.parameters() == [0, 0] => {
          state.lock().unwrap().tv = Some(true);
            println!("======== TV an ===========")
        },
        CecOpcode::ReportAudioStatus if cmd.initiator() == CecLogicalAddress::Audiosystem => {
            /*
Used to indicate the current audio volume status of a device.
N indicates audio playback volume, expressed as a percentage
(0% - 100%). N=0 is no sound; N=100 is maximum volume
sound level.
The linearity of the sound level is device dependent.
This value is mainly used for displaying a volume status bar on
a TV screen.
*/
                let v = cmd.parameters()[0];
                println!("Muted: {}", v & 0x80);
                println!("Vol: {}%", v & 0x7f);
        },
        CecOpcode::VendorCommandWithId if cmd.parameters() == [8, 0,70,0,19, 0, 16, 0, 0, 2, 0, 0, 0, 0] => {
          println!("≈========tv realy on=========");
        },
        CecOpcode::SetSystemAudioMode => {
          let mut s = state.lock().unwrap();
          s.audio_mode = cmd.parameters().first().map(|&b|b==1);
          s.avr_standby = Some(false);
        },
        CecOpcode::ReportPowerStatus if cmd.initiator() == CecLogicalAddress::Audiosystem => {
          state.lock().unwrap().avr_standby =
            match cmd.parameters().first() {
                Some(1) /*| Some(3)*/ => {
                    //standby
                    println!("Updated AVR PWR: Some(true) -> standby");
                    Some(true)
                },
                Some(0) /*| Some(2)*/ => {
                    //on
                    println!("Updated AVR PWR: Some(false) -> on");
                    Some(false)
                },
                _ => {
                    println!("Updated AVR PWR: None");
                    None
                }
            };
        },
        CecOpcode::ReportPhysicalAddr if cmd.initiator() == CecLogicalAddress::Audiosystem && cmd.parameters() == [0x30, 0, 5] => {
            //audio became ready to receive commands
            state.lock().unwrap().avr_ready = true;
        },
        CecOpcode::GiveDevicePowerStatus if cmd.initiator() == CecLogicalAddress::Tv && cmd.destination() == CecLogicalAddress::Playback2 => {
           //libCEC answers on its own
        },
        _=>{
                println!("<6>cec cmd: {:?} -> {:?}   {:?}: {:x?}",cmd.initiator(), cmd.destination(), opcode, cmd.parameters());
           }
    }
}

fn pulse_callback(res: ListResult<&SinkInputInfo> ) {
    static mut PULSE_SINKS: usize = 0;
    match res {                
        ListResult::Item(item) => {
            if !item.corked {
                unsafe {PULSE_SINKS += 1;}
                println!("<6>pulse: {:?}", item.proplist);
                /*
                println!("<6>pulse: {:?} {:?}%", item.proplist, item.volume.get().get(0).map(|v|100*v.0/0xFFFF));
                if let Some(v) = item.volume.get().get(0) {
                   unsafe{PULSE_VOLUME = (100 * (v.0 as u32)  / 0xFFFF) as u8;}
                }*/
/*
SinkInputInfo { index: 61, name: Some("Playback"), owner_module: Some(11), client: Some(83), sink: 0, sample_spec: Spec { format: S16le, rate: 44100, channels: 2 }, channel_map: Map { channels: 2, map: [FrontLeft, FrontRight, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono] }, volume: ChannelVolumes { channels: 2, values: [Volume(65536), Volume(65536), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0)] }, buffer_usec: MicroSeconds(89886), sink_usec: MicroSeconds(55105), resample_method: None, driver: Some("protocol-native.c"), mute: false, proplist: [media.name = "Playback", application.name = "Snapcast", native-protocol.peer = "UNIX socket client", native-protocol.version = "34", application.icon_name = "snapcast", media.role = "music", application.process.id = "2617289", application.process.user = "pi", application.process.host = "livax", application.process.binary = "snapclient", application.language = "C", window.x11.display = ":0", application.process.machine_id = "18141558705a42f68ed370b7eaac3e4d", module-stream-restore.id = "sink-input-by-media-role:music"], corked: false, has_volume: true, volume_writable: true, format: Info { encoding: PCM, properties: [format.sample_format = "\"s16le\"", format.rate = "44100", format.channels = "2", format.channel_map = "\"front-left,front-right\""] } }
*/
            }
        },
        ListResult::End | ListResult::Error => {
            unsafe{
                PULSE_PLAYS = PULSE_SINKS >0;
            };
            unsafe{PULSE_SINKS = 0;}
        },
    }
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
    print_err(cec.transmit_data(from, CecLogicalAddress::Audiosystem, CecOpcode::SystemAudioModeRequest, b"\x33\x00"),"SystemAudioModeRequest failed");
    //print_err(cec.audio_get_status(),"GiveAudioStatus");
}
///Print an error
fn print_err<E: std::fmt::Debug>(res: Result<(),E>, name: &str)
{
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
    print_err(cec.transmit(from, CecLogicalAddress::Audiosystem, CecOpcode::SystemAudioModeRequest),"SystemAudioModeRequest off");
}
/*
fn sync_volume(cec: &CecConnection, pwr: &GlobalSiSPM) {
 if !unsafe{PULSE_PLAYS} {return;}
 let cec = unsafe {CEC.as_ref()}.expect("no cec");
 loop {
  let (vcec, vpulse) = unsafe{(CEC_VOLUME, PULSE_VOLUME)};
  if vcec+1 >= vpulse && vcec <= vpulse+1 {return;}
  let key = if vcec < vpulse {
    unsafe{CEC_VOLUME+=1;}
    CecUserControlCode::VolumeUp
  }else{
    unsafe{CEC_VOLUME-=1;}
    CecUserControlCode::VolumeDown
  };
  print_err(cec.send_keypress(CecLogicalAddress::Audiosystem, key, true),"key press");
  print_err(cec.send_key_release(CecLogicalAddress::Audiosystem, true),"key release");
  break;
 }

 print_err(cec.audio_get_status(),"audio_get_status"); // -> <Give Audio Status>

}
*/
