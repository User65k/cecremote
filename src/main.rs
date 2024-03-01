use cec_rs::{CecConnectionCfgBuilder, CecLogicalAddress, CecCommand, CecLogMessage, CecDeviceType, CecDeviceTypeVec, CecKeypress, CecOpcode, CecDatapacket, CecConnection, CecLogicalAddresses};
use libpulse_binding::{context::introspect::SinkInputInfo, callbacks::ListResult};
use std::{thread, time,boxed::Box};
use std::convert::TryInto;
use sispm::{get_devices, GlobalSiSPM};
use std::time::Duration;

mod pulse;
///Strg+C was received
static mut DIE: bool = false;
static mut PULSE_PLAYS: bool = false;

fn main() {
    ctrlc::set_handler(move || {
        unsafe {DIE = true;}
    })
    .expect("Error setting Ctrl-C handler");

    let cec_bus = CecConnectionCfgBuilder::default()
        .port("RPI".to_string())
        .device_name("pi4".to_string())
        .device_types(CecDeviceTypeVec::new(CecDeviceType::PlaybackDevice))
        .base_device(CecLogicalAddress::Audiosystem).hdmi_port(3)
        .physical_address(0x3300)
        .command_received_callback(Box::new(command))
        .log_message_callback(Box::new(log))
        .key_press_callback(Box::new(key))
        .wake_devices(CecLogicalAddresses::default())
        .build().expect("cec build").open().expect("cec open");

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

    let one_sec = time::Duration::from_millis(SLEEP_TIME_CYCLE_MS);
    let mut cycles_not_changed = 0;
    while !(unsafe {DIE}) {
        let (
            tv,
            pulse,
            avr_ready,
            avr_standby
        ) = unsafe{(
            TV_STATUS,
            PULSE_PLAYS,
            AVR_READY,
            AVR_STANDBY
        )};
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
                cec_audio_mode_off(&cec_bus);
                switch_light(&pwr_socket, true);
                MediaState::Watching
            },
            MediaState::Playing if !pulse => {
                println!("Playing: {tv:?} {pulse}");
                // Audio turned Off
//TODO restore vol
                cec_audio_mode_off(&cec_bus);
                switch_subwoofer(&pwr_socket, false);
                MediaState::Off
            },
            MediaState::Off if pulse || tv == Some(true) => {
                println!("Off: {tv:?} {pulse}");
                // Turn On
                cycles_not_changed = 0;
                switch_avr(&pwr_socket, true);
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
                }else{
                    if pulse {
                        //TODO store volume
                        cec_audio_mode(&cec_bus);
                        print_err(cec_bus.get_device_power_status(CecLogicalAddress::Audiosystem),"Request AVR stadby state");
                        MediaState::Playing
                    }else{
                        println!("<3>TV and Audio off. No need for AVR anymore");
                        MediaState::Off
                    }
                }
            },
            MediaState::WaitForAudio if cycles_not_changed == THREE_SEC_IN_CYCLES => {
                //AVR wont turn on but has power
                println!("<4>WaitForAudio takes too long");
                if tv == Some(true) {
                    print_err(cec_bus.send_power_on_devices(CecLogicalAddress::Audiosystem),"PwrOn audio");
                }else{
                    if pulse {
                        cec_audio_mode(&cec_bus);
                    }
                }
                cycles_not_changed += 1;
                MediaState::WaitForAudio
            },
            MediaState::Watching if avr_standby != Some(false) => {
                println!("<5>Watching: avr standby: {avr_standby:?}");
                //TV is running but audio is off
                print_err(cec_bus.send_power_on_devices(CecLogicalAddress::Audiosystem),"PwrOn audio");
                //TODO request PWR state?
                print_err(cec_bus.get_device_power_status(CecLogicalAddress::Audiosystem),"Request AVR stadby state");
                MediaState::Watching
            },
            MediaState::Playing if avr_standby != Some(false) => {
            // none -> ask for standby status
                println!("<5>Playing: avr standby: {avr_standby:?}"); //None -> is not the reason the TV turns on
                //Pulse is running but audio is off
                cec_audio_mode(&cec_bus);
                print_err(cec_bus.get_device_power_status(CecLogicalAddress::Audiosystem),"Request AVR stadby state");
                MediaState::Playing
            },
            MediaState::AVRHasPwr => {
                println!("AVRHasPwr: {avr_standby:?}");
                match avr_standby {
                  None => {
                    // Seriice started, dont know whats up
                    print_err(cec_bus.get_device_power_status(CecLogicalAddress::Audiosystem),"Request AVR stadby state");
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
                        switch_avr(&pwr_socket, false);
                    }else{
                        //send AVR to standby first
                        print_err(cec_bus.send_standby_devices(CecLogicalAddress::Audiosystem),"SendStandbyDevices audio");
                        print_err(cec_bus.get_device_power_status(CecLogicalAddress::Audiosystem),"Request AVR stadby state");
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
        thread::sleep(one_sec);
    }
    println!("Bye");
}
const SLEEP_TIME_CYCLE_MS: u64 = 250;
const FOUR_SEC_IN_CYCLES: u8 = (4_000/SLEEP_TIME_CYCLE_MS) as u8;
const THREE_SEC_IN_CYCLES: u8 = (3_000/SLEEP_TIME_CYCLE_MS) as u8;

#[inline]
fn switch_light(pwr_socket: &GlobalSiSPM, on: bool) {
    print_err(pwr_socket.set_status(1, on),"pwr1");
}
#[inline]
fn switch_avr(pwr_socket: &GlobalSiSPM, on: bool) {
    unsafe {
        AVR_READY=false;
        AVR_STANDBY=None;
        AUDIO_MODE=None;
    }
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
/// TV is playing
static mut TV_STATUS: Option<bool> = None;
static mut AUDIO_MODE: Option<bool> = None;
static mut AVR_READY:bool=false;
static mut AVR_STANDBY:Option<bool>=None;

fn command(cmd: CecCommand) {
    match cmd.opcode {
        CecOpcode::Standby if cmd.initiator == CecLogicalAddress::Tv => {
            unsafe {TV_STATUS=Some(false);}
            println!("======== Tv aus ===========")
        },
        CecOpcode::ActiveSource if cmd.initiator == CecLogicalAddress::Tv && cmd.parameters == slice_to_arr(&[0, 0]) => {
            unsafe {TV_STATUS=Some(true);}
            println!("======== TV an ===========")
        },
        CecOpcode::ReportAudioStatus if cmd.initiator == CecLogicalAddress::Audiosystem => {
            /*
Used to indicate the current audio volume status of a device.
N indicates audio playback volume, expressed as a percentage
(0% - 100%). N=0 is no sound; N=100 is maximum volume
sound level.
The linearity of the sound level is device dependent.
This value is mainly used for displaying a volume status bar on
a TV screen.
*/
                let v = cmd.parameters.0[0];
                println!("Muted: {}", v & 0x80);
                println!("Vol: {}%", v & 0x7f);
        },
        CecOpcode::VendorCommandWithId if cmd.parameters == slice_to_arr(&[8, 0,70,0,19, 0, 16, 0, 0, 2, 0, 0, 0, 0]) => {
          println!("≈========tv realy on=========");
        },
        CecOpcode::SetSystemAudioMode => {
            unsafe {AUDIO_MODE=cmd.parameters.0.get(0).map(|&b|b==1);AVR_STANDBY=Some(false);}
        },
        CecOpcode::ReportPowerStatus if cmd.initiator == CecLogicalAddress::Audiosystem => {
            match cmd.parameters.0.get(0) {
                Some(1) /*| Some(3)*/ => {
                    //standby
                    println!("Updated AVR PWR: Some(true) -> standby");
                    unsafe {AVR_STANDBY=Some(true);}
                },
                Some(0) /*| Some(2)*/ => {
                    //on
                    println!("Updated AVR PWR: Some(false) -> on");
                    unsafe {AVR_STANDBY=Some(false);}
                },
                _ => {
                    println!("Updated AVR PWR: None");
                    unsafe {AVR_STANDBY=None;}
                }
            }            
        },
        CecOpcode::ReportPhysicalAddress if cmd.initiator == CecLogicalAddress::Audiosystem && cmd.parameters == slice_to_arr(&[0x30, 0, 5]) => {
            //audio became ready to receive commands
            unsafe {AVR_READY=true;}
        },
        CecOpcode::GiveDevicePowerStatus if cmd.initiator == CecLogicalAddress::Tv && cmd.destination == CecLogicalAddress::Playbackdevice2 => {
           //libCEC answers on its own
        },
        _=>{
                println!("<6>cec cmd: {:?} -> {:?}   {:?}: {:?}",cmd.initiator, cmd.destination, cmd.opcode, cmd.parameters);
           }
    }
}

fn pulse_callback(res: ListResult<&SinkInputInfo> ) {
    static mut PULSE_SINKS: usize = 0;
    match res {                
        ListResult::Item(item) => {
            if !item.corked {
                unsafe {PULSE_SINKS += 1;}/*
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


fn log(msg: CecLogMessage) {
/*
Dec 15 22:24:30 livax cecremote[3077436]: Debug:<< Playback 2 (8) -> TV (0): on
Dec 15 22:24:30 livax cecremote[3077436]: Traffic:<< 80:90:00
Dec 15 22:24:30 livax cecremote[3077436]: Debug:>> TV (0) -> Playback 2 (8): give device power status (8F)
Dec 15 22:25:30 livax cecremote[3077436]: Traffic:>> 08:8f
*/
    match msg.level {
        cec_rs::CecLogLevel::Traffic => {return;},
        cec_rs::CecLogLevel::Debug => {
         if msg.message.ends_with("give device power status (8F)")
         || msg.message.ends_with("Playback 2 (8) -> TV (0): on") {return;}
        },
        _ => {},
    }
    println!("<7>{}:{}", msg.level, msg.message);
}
fn key(key: CecKeypress) {
 println!("key {:?} {:?}", key.keycode, key.duration);
}
fn slice_to_arr(d: &[u8]) -> CecDatapacket {
 CecDatapacket(d.try_into().expect("data bigger than 64 bytes"))
}

/// requests audio focus. Turn on AVR if needed
fn cec_audio_mode(cec: &CecConnection) {
    /*
    #The feature can be initiated from a device (eg TV or STB) or the amplifier. In the case of initiation by a device
    #other than the amplifier, that device sends an <System Audio Mode Request> to the amplifier, with the
    #physical address of the device that it wants to use as a source as an operand. Note that the Physical Address
    #may be the TV or STB itself.

The amplifier comes out of standby (if necessary) and switches to the relevant connector for device specified by [Physical Address].
It then sends a <Set System Audio Mode> [On] message.

...  the device requesting this information can send the volume-related <User Control Pressed> or <User Control Released> messages.
*/
    let ccmd = CecCommand {
        initiator: CecLogicalAddress::Playbackdevice2,
        destination: CecLogicalAddress::Audiosystem,
        ack: false,
        eom: true,
        opcode: CecOpcode::SystemAudioModeRequest,
        parameters: slice_to_arr(b"\x33\x00"),
        opcode_set: true,
        transmit_timeout: Duration::from_millis(500),
    };
    print_err(cec.transmit(ccmd),"SystemAudioModeRequest failed");
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
fn cec_audio_mode_off(cec: &CecConnection) {
/*
<System Audio Mode Request> sent without a [Physical Address] parameter requests termination of the feature.
In this case, the amplifier sends a <Set System Audio Mode> [Off] message.
    */
    let ccmd = CecCommand {
        initiator: CecLogicalAddress::Playbackdevice2,
        destination: CecLogicalAddress::Audiosystem,
        ack: false,
        eom: true,
        opcode: CecOpcode::SystemAudioModeRequest,
        parameters: slice_to_arr(b""),
        opcode_set: true,
        transmit_timeout: Duration::from_millis(500),
    };
    print_err(cec.transmit(ccmd),"SystemAudioModeRequest off");
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
