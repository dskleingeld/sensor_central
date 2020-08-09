#![cfg(feature = "ble")]

use std::time::{Duration, Instant};
use std::fs::{File};
use std::io::Read;
use std::thread;
use crossbeam_channel::Sender;
use std::os::unix::io::{RawFd, FromRawFd};

use bluebus::{BleBuilder, Ble};
use nix::poll::{poll, PollFd, PollFlags};

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{Aead, NewAead, generic_array::GenericArray};
use rand::{thread_rng, Rng};

use crate::error::Error;
use crate::SensorValue;

mod localisation;
use localisation::{DeviceInfo, UuidInfo, SENSORS};
mod error;
use error::ConnectionError;

const NONCE_CHAR_UUID: &'static str = "93700004-1bb7-1599-985b-f5e7dc991483";
const PIN_CHAR_UUID: &'static str = "93700005-1bb7-1599-985b-f5e7dc991483";


pub struct Key {
    array: [u8;16],
}

impl std::str::FromStr for Key {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let s = s.trim_start_matches("[");
        let s = s.trim_end_matches("]");

        let mut array = [0u8;16];
        for (byte, numb) in array.iter_mut().zip(s.split(",")){
            *byte = numb.parse::<u8>()?;
        }
        Ok(Self{array})
    }
}

pub struct BleSensors {
    disconnected: Vec<DeviceInfo>,
    connected: Vec<DeviceInfo>,
    notify_streams: NotifyStreams,

    ble: Ble,
    key: [u8;16],
}

fn has_io(pollable: &PollFd) -> bool {
    if let Some(poll_res) = pollable.revents() {
        if poll_res.contains(PollFlags::POLLIN) {
            return true;
        }
    }
    false
}

#[derive(Default)]
struct NotifyStreams {
    pollables: Vec<PollFd>,
    files: Vec<File>,
    infos: Vec<UuidInfo>,
}

impl NotifyStreams {
    pub fn add(&mut self, fds: Vec<RawFd>, device: &DeviceInfo){
        
        self.pollables.extend(fds.iter()
            .map(|fd| PollFd::new(*fd, PollFlags::POLLIN)));
        self.files.extend(fds.iter()
            .map(|fd| unsafe {File::from_raw_fd(*fd)} ));
        //self.fds.append(fds);es;

        self.infos.extend(device.values.iter().cloned());
    }

    pub fn remove(&mut self, device: &DeviceInfo){
        let mut start = None;
        for (i,info) in self.infos.iter().enumerate() {
            if info == device.values.first().unwrap() {
                start = Some(i);
            }
        } 

        let start = start.expect("device should be in infos list!");
        for i in start..start+device.values.len() {
            self.pollables.remove(i);
            self.files.remove(i);
            self.infos.remove(i);
        }
    }

    pub fn handle(&mut self, buffer: &mut [u8], s: &mut Sender<SensorValue>){
        if poll(&mut self.pollables, -1).unwrap() < 1 {
            dbg!("poll() failed"); //TODO improve on this
        }

        for (i, pollable) in self.pollables.iter().enumerate() {
            if !has_io(pollable) {continue;}

            let info = &self.infos[i];
            self.files[i].read(&mut buffer[..info.byte_len()]).unwrap();
            info.process(&buffer, s);
        }
    }
}

const PAIRING_TIMEOUT: Duration = Duration::from_secs(15);
impl BleSensors {
    pub fn new(ble_key: Key) -> Result<Self, Error> {
        let mut ble = BleBuilder::new().build()?;
        ble.start_discovery()?;
        thread::sleep(Duration::from_secs(5));
        ble.stop_discovery()?;

        Ok(BleSensors{
            disconnected: SENSORS.to_vec(),
            connected: Vec::new(),
            
            notify_streams: NotifyStreams::default(),
            
            ble,
            key: ble_key.array,
        })
    }

    pub fn reconnect_disconnected(&mut self) -> Result<(), ConnectionError> {
        let connected = &mut self.connected;
        let disconnected = &mut self.disconnected;
        let notify_streams = &mut self.notify_streams;
        let ble = &mut self.ble;
        let key = &self.key;

        disconnected.drain_filter(|device| {
            let res = Self::connect_device(ble, device, key);
            match res {
                Ok(fds) => {
                    connected.push(device.clone());
                    notify_streams.add(fds, device);
                    true //remove device from disconnected
                }
                Err(e) => {
                    if !e.is_recoverable() {
                        panic!("ran into unrecoverable error during connection of device: {}, error was: {:?}", device.adress, e);
                    }
                    false //keep device in disconnected
                }
            }
        });
        Ok(())
    }

    fn connect_device(ble: &mut Ble, device: &DeviceInfo, key: &[u8;16])
     -> Result<Vec<RawFd>, ConnectionError> {
    
        ble.connect(device.adress)?;
        let get_key = Self::setup_key(ble, device.adress, key)?;
        ble.pair(device.adress, get_key, PAIRING_TIMEOUT)?;
        
        let test: Result<Vec<RawFd>, bluebus::Error> = device.values
            .iter()
            .map(|u| ble.notify(device.adress, u.uuid))
            .collect();
        let test = test?; //TODO cleanup
        Ok(test)
    }

    fn setup_key(ble: &mut Ble, adress: &str, key: &[u8;16]) -> Result<impl Fn() -> u32, bluebus::Error> {
        const NONCE_SIZE: usize = 12;
        const MAC_SIZE: usize = 6;

        let mut rng = rand::thread_rng();

        let cipher = Aes128Gcm::new(GenericArray::from_slice(key));
        
        let mut nonce = [0u8; NONCE_SIZE];
        rng.fill(&mut nonce[..]);
        let nonce = GenericArray::from_slice(&nonce);

        let pin: u32 = rng.gen_range(0, 999999);
        let mut pin_array = [0u8; 4];
        pin_array[..4].copy_from_slice(&pin.to_be_bytes());

        let ciphertext = cipher.encrypt(nonce, pin_array.as_ref())
        .expect("encryption failure!"); // NOTE: handle this error to avoid panics!
        
        ble.write(adress, NONCE_CHAR_UUID, nonce)?;
        ble.write(adress, PIN_CHAR_UUID, ciphertext)?;
        let get_key = move || pin;
        Ok(get_key)
    }

    fn check_for_disconnects(&mut self) {
        let connected = &mut self.connected;
        let notify_streams = &mut self.notify_streams;
        let ble = &mut self.ble;

        connected.drain_filter(|device| {
            if ble.is_connected(device.adress).unwrap() {
                false
            } else {
                notify_streams.remove(device);
                true
            }
        });
    }
    
    pub fn handle(&mut self, mut s: Sender<SensorValue>) {
        const TIMEOUT: Duration = Duration::from_secs(5);
        let mut buffer = [0u8; 100];
        let now = Instant::now();

        loop {
            if now.elapsed() > TIMEOUT {
                self.check_for_disconnects();
            }

            self.reconnect_disconnected().unwrap();
            self.notify_streams.handle(&mut buffer, &mut s);
        }
    }
}

pub fn start_monitoring(s: Sender<SensorValue>, ble_key: Key) {
    thread::spawn(move || {
        let mut sensors = BleSensors::new(ble_key).unwrap();
        sensors.handle(s);
    });
}