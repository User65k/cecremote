use cec_rs::{CecConnectionCfgBuilder, CecLogicalAddress, CecCommand, CecLogMessage, CecDeviceType, CecDeviceTypeVec, CecKeypress, CecOpcode, CecDatapacket, CecConnection, CecUserControlCode};
use std::{thread, time,boxed::Box};
use std::convert::TryInto;
use sispm::{get_devices, GlobalSiSPM};
use std::time::Duration;

mod pulse;

static mut PWR_SOCKET: Option<GlobalSiSPM> = None;
static mut CEC: Option<CecConnection> = None;
static mut CEC_VOLUME: u8 = 0;
static mut DIE: bool = false;
static mut TV_STATUS: bool = false;
static mut PULSE_SINKS: u32 = 0;
static mut PULSE_PLAYS: bool = false;
static mut PULSE_VOLUME: u8 = 0;
/*
1: Licht
2: Audio
3: Subwoofer
*/

fn main() {
    ctrlc::set_handler(move || {
        unsafe {DIE = true;}
    })
    .expect("Error setting Ctrl-C handler");

    let b = CecConnectionCfgBuilder::default()
             .port("RPI".to_string())
             .device_name("pi4".to_string())
             .device_types(CecDeviceTypeVec::new(CecDeviceType::PlaybackDevice));
    let b = b.base_device(CecLogicalAddress::Audiosystem).hdmi_port(3);
    let b = b.physical_address(0x3300);
    let b = b.command_received_callback(Box::new(command))
            .log_message_callback(Box::new(log))
            .key_press_callback(Box::new(key));
    let bus = b.build().expect("cec build").open().expect("cec open");

    unsafe {CEC = Some(bus)};
    unsafe {PWR_SOCKET = get_devices().expect("on pwr socket").pop()};
    
//    println!("Hello, world! {:?}", unsafe {PWR_SOCKET.as_ref()});

    unsafe {if let None = PWR_SOCKET {return;}}

    //monitor TV status. Start based on light status
    unsafe {TV_STATUS = PWR_SOCKET.as_ref().unwrap().get_status(1).expect("status?");}

    //monitor audio status
    let _a = pulse::NotWhenAudio::new(|ctx: &mut libpulse_binding::context::Context| {
        ctx.introspect()
            .get_sink_input_info_list(move |res| match res {
                libpulse_binding::callbacks::ListResult::Item(item) => {
                    if !item.corked {
                        unsafe{PULSE_SINKS += 1;}
                        println!("<6>pulse: {:?} {:?}%", item.proplist, item.volume.get().get(0).map(|v|100*v.0/0xFFFF));
		        if let Some(v) = item.volume.get().get(0) {
                           unsafe{PULSE_VOLUME = (100 * (v.0 as u32)  / 0xFFFF) as u8;}
                        }
/*
SinkInputInfo { index: 61, name: Some("Playback"), owner_module: Some(11), client: Some(83), sink: 0, sample_spec: Spec { format: S16le, rate: 44100, channels: 2 }, channel_map: Map { channels: 2, map: [FrontLeft, FrontRight, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono, Mono] }, volume: ChannelVolumes { channels: 2, values: [Volume(65536), Volume(65536), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0), Volume(0)] }, buffer_usec: MicroSeconds(89886), sink_usec: MicroSeconds(55105), resample_method: None, driver: Some("protocol-native.c"), mute: false, proplist: [media.name = "Playback", application.name = "Snapcast", native-protocol.peer = "UNIX socket client", native-protocol.version = "34", application.icon_name = "snapcast", media.role = "music", application.process.id = "2617289", application.process.user = "pi", application.process.host = "livax", application.process.binary = "snapclient", application.language = "C", window.x11.display = ":0", application.process.machine_id = "18141558705a42f68ed370b7eaac3e4d", module-stream-restore.id = "sink-input-by-media-role:music"], corked: false, has_volume: true, volume_writable: true, format: Info { encoding: PCM, properties: [format.sample_format = "\"s16le\"", format.rate = "44100", format.channels = "2", format.channel_map = "\"front-left,front-right\""] } }
*/
                    }
                },
                libpulse_binding::callbacks::ListResult::End | libpulse_binding::callbacks::ListResult::Error => {
                    let (audio, tv, audo_was_on) = unsafe{
                        let was = PULSE_PLAYS;
                        PULSE_PLAYS = PULSE_SINKS >0;
                        PULSE_SINKS = 0;
                        (PULSE_PLAYS, TV_STATUS, was)
                    };
                    
                    if !tv {
			if audio != audo_was_on {
                        	if audio {
                                    if let Some(pwr) = unsafe {PWR_SOCKET.as_ref()} {
				        print_err(pwr.set_status(2, true),"pwr2 on");
				    }
                	            //cec_audio_mode();
        	                }else{
	                            cec_audio_mode_off();
                        	}
			}else{
				//only volume changed
				sync_volume();
			}
                    }
                },
            });
    }).expect("pulse watcher");

    let one_sec = time::Duration::from_millis(1000);
    while !(unsafe {DIE}) {
        thread::sleep(one_sec);
    }
    println!("Bye");
    unsafe {CEC=None};
    unsafe {PWR_SOCKET=None;}
/*
change to audio source (TV+PS4 off)

wait for PS4 off -> change audio to TV
*/
    /*
bus.volume_up(send_release: bool)?;
bus.volume_down(send_release: bool)?;
bus.audio_mute()?;
bus.audio_unmute()?;
bus.set_inactive_view()?;
    */

}

fn command(cmd: CecCommand) {
    match cmd.opcode {
        CecOpcode::Standby if cmd.initiator == CecLogicalAddress::Tv => {
            if let Some(pwr) = unsafe {PWR_SOCKET.as_ref()} {
                unsafe {TV_STATUS=false;}
                print_err(pwr.set_status(1, false),"pwr1 off");
                //wait half a sec
                thread::sleep(Duration::from_millis(500));
                print_err(pwr.set_status(2, false),"pwr2 off");
                print_err(pwr.set_status(3, false),"pwr3 off");
            }
            println!("======== Tv aus ===========")
        },
        CecOpcode::ActiveSource if cmd.initiator == CecLogicalAddress::Tv && cmd.parameters == sliceToArr(&[0, 0]) && !unsafe {TV_STATUS} => {
            if let Some(pwr) = unsafe {PWR_SOCKET.as_ref()} {
                unsafe {TV_STATUS=true;}
                print_err(pwr.set_status(2, true),"pwr2 on");
                print_err(pwr.set_status(3, true),"pwr3 on");
                print_err(pwr.set_status(1, true),"pwr1 on");
                thread::sleep(Duration::from_millis(500));
                let cec = unsafe {CEC.as_ref()}.expect("no cec");
                //cec.set_active_source(CecDeviceType::Tv);
                while let Err(_) = cec.send_power_on_devices(CecLogicalAddress::Audiosystem) {
                   thread::sleep(Duration::from_millis(500));
                }
            }
            println!("======== TV an ===========")
        },
        CecOpcode::VendorCommandWithId if cmd.parameters == sliceToArr(&[8, 0, 70, 0,19, 0, 16, 0, 0, 2, 0, 0, 0, 0]) => {
// CecDatapacket([8, 0, 70, 0,19, 0, 16, 0, 0, 2, 0, 0, 0, 0])
println!("≈========tv realy on=========");
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
                unsafe{CEC_VOLUME= v & 0x7f;}
                sync_volume();
        },
        CecOpcode::ReportPhysicalAddress if cmd.initiator == CecLogicalAddress::Audiosystem && cmd.parameters == sliceToArr(&[0x30, 0, 5]) => {
        	//audio became ready to receive commands
		if unsafe {PULSE_PLAYS && !TV_STATUS} {
			cec_audio_mode();
		}
        },
        CecOpcode::GiveDevicePowerStatus if cmd.initiator == CecLogicalAddress::Tv && cmd.destination == CecLogicalAddress::Playbackdevice2 => {
           //libCEC answers on its own
        },
        _=>{
                println!("<6>cec cmd: {:?} -> {:?}   {:?}: {:?}",cmd.initiator, cmd.destination, cmd.opcode, cmd.parameters);
           }
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
fn sliceToArr(d: &[u8]) -> CecDatapacket {
 CecDatapacket(d.try_into().expect("data bigger than 64 bytes"))
}

fn cec_audio_mode() {
    /*if let Some(pwr) = unsafe {PWR_SOCKET.as_ref()} {
        print_err(pwr.set_status(2, true),"pwr2 on");
    }*/
    let cec = unsafe {CEC.as_ref()}.expect("no cec");
    //thread::sleep(Duration::from_millis(2000));
    //cec.send_power_on_devices(CecLogicalAddress::Audiosystem);
    //thread::sleep(Duration::from_millis(500));
    /*
    #The feature can be initiated from a device (eg TV or STB) or the amplifier. In the case of initiation by a device
    #other than the amplifier, that device sends an <System Audio Mode Request> to the amplifier, with the
    #physical address of the device that it wants to use as a source as an operand. Note that the Physical Address
    #may be the TV or STB itself.

The amplifier comes out of standby (if necessary) and switches to the relevant connector for device specified by [Physical Address].
It then sends a <Set System Audio Mode> [On] message.

...  the device requesting this information can send the volume-related <User Control Pressed> or <User Control Released> messages.
*/
    loop {
     let ccmd = CecCommand {
        initiator: CecLogicalAddress::Playbackdevice2,
        destination: CecLogicalAddress::Audiosystem,
        ack: false,
        eom: true,
        opcode: CecOpcode::SystemAudioModeRequest,
        parameters: sliceToArr(b"\x33\x00"),
        opcode_set: true,
        transmit_timeout: Duration::from_millis(500),
     };
     if let Ok(_) = cec.transmit(ccmd) {break;}
     eprintln!("<3>Err: SystemAudioModeRequest failed");
     thread::sleep(Duration::from_millis(500));
    }
    print_err(cec.audio_get_status(),"GiveAudioStatus");
}
fn print_err<E: std::fmt::Debug>(res: Result<(),E>, name: &str)
{
	if let Err(e) = res {
	        eprintln!("<3>{} Err: {:?}", name, e);
	}
}
fn cec_audio_mode_off() {
   let cec = unsafe {CEC.as_ref()}.expect("no cec");
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
        parameters: sliceToArr(b""),
        opcode_set: true,
        transmit_timeout: Duration::from_millis(500),
    };
    print!("sent SystemAudioModeRequest {:?}", ccmd.parameters);
    print_err(cec.transmit(ccmd),"SystemAudioModeRequest off");
    
   thread::sleep(Duration::from_millis(500));
   print_err(cec.send_standby_devices(CecLogicalAddress::Audiosystem),"SendStandbyDevices audio");
   thread::sleep(Duration::from_millis(500));
   if let Some(pwr) = unsafe {PWR_SOCKET.as_ref()} {
         print_err(pwr.set_status(2, false),"pwr2 off");
   }
}

fn sync_volume() {
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
