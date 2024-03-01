use crate::GState;
use cec_linux::{
    CecDevice, CecEvent, CecLogAddrMask, CecLogicalAddress, CecMsg, CecOpcode, PollFlags,
};
use std::sync::{Arc, Mutex};
use std::process::Command;

pub fn mon(cec_mon: CecDevice, mut mutex: Arc<Mutex<GState>>) {
    loop {
        let f = cec_mon
            .poll(
                PollFlags::POLLIN | PollFlags::POLLRDNORM | PollFlags::POLLPRI,
                -1,
            )
            .unwrap();
        if f.intersects(PollFlags::POLLPRI) {
            if let CecEvent::StateChange(s) = cec_mon.get_event().unwrap() {
                println!("<7>{:?}", s);
                let on =
                if s.log_addr_mask.contains(CecLogAddrMask::Playback1) {
                    mutex.lock().unwrap().cec_addr = Some(CecLogicalAddress::Playback1);
                    true
                } else if s.log_addr_mask.contains(CecLogAddrMask::Playback2) {
                    mutex.lock().unwrap().cec_addr = Some(CecLogicalAddress::Playback2);
                    true
                } else if s.log_addr_mask.contains(CecLogAddrMask::Playback3) {
                    mutex.lock().unwrap().cec_addr = Some(CecLogicalAddress::Playback3);
                    true
                } else {
                    mutex.lock().unwrap().cec_addr = None;
                    false
                };
                if on {
                    let runs = Command::new("systemctl")
                        .args(["--user", "is-active", "pipewire"])
                        .output()
                        .expect("exec").stdout == b"active\n";
                    if !runs {
                        Command::new("systemctl")
                            .args(["--user", "start", "pipewire"])
                            .status().expect("restart");
                    }
                }
            }
        }
        if f.contains(PollFlags::POLLIN | PollFlags::POLLRDNORM) {
            let msg = cec_mon.rec().unwrap();
            command(msg, &mut mutex)
        }
    }
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
        }
        CecOpcode::ActiveSource
            if cmd.initiator() == CecLogicalAddress::Tv && cmd.parameters() == [0, 0] =>
        {
            state.lock().unwrap().tv = Some(true);
            println!("======== TV an ===========")
        }
        /*CecOpcode::ReportAudioStatus if cmd.initiator() == CecLogicalAddress::Audiosystem => {
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
        }*/
        CecOpcode::VendorCommandWithId
            if cmd.parameters() == [8, 0, 70, 0, 19, 0, 16, 0, 0, 2, 0, 0, 0, 0] =>
        {
            println!("≈========tv realy on=========");
        }
        CecOpcode::SetSystemAudioMode => {
            let mut s = state.lock().unwrap();
            s.audio_mode = cmd.parameters().first().map(|&b| b == 1);
            s.avr_standby = Some(false);
        }
        CecOpcode::ReportPowerStatus if cmd.initiator() == CecLogicalAddress::Audiosystem => {
            state.lock().unwrap().avr_standby = match cmd.parameters().first() {
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
        }
        CecOpcode::ReportPhysicalAddr
            if cmd.initiator() == CecLogicalAddress::Audiosystem
                && cmd.parameters() == [0x30, 0, 5] =>
        {
            //audio became ready to receive commands
            state.lock().unwrap().avr_ready = true;
        }
        CecOpcode::GiveDevicePowerStatus
            if cmd.initiator() == CecLogicalAddress::Tv
                && cmd.destination() == CecLogicalAddress::Playback2 =>
        {
            //libCEC answers on its own
        }
        CecOpcode::UserControlPressed | CecOpcode::UserControlReleased
            if cmd.initiator() == CecLogicalAddress::Playback2 => {}
        CecOpcode::FeatureAbort if cmd.initiator() == CecLogicalAddress::Playback2 => {}//vendor id
        _ => {
            println!(
                "<7>cec cmd: {:?} -> {:?}   {:?}: {:x?}",
                cmd.initiator(),
                cmd.destination(),
                opcode,
                cmd.parameters()
            );
        }
    }
}
