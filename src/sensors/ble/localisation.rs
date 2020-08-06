use bitspec::Field;
use crate::{SensorValue, Sensor};
use crossbeam_channel::Sender;

#[derive(Clone)]
pub struct DeviceInfo {
    pub adress: &'static str,
    pub values: &'static [UuidInfo],
}

#[derive(Clone)]
pub struct UuidInfo {
    pub uuid: &'static str,
    fields: &'static [Field<f32>],
    pub ha_values: &'static [Sensor],  
}

impl PartialEq for UuidInfo {
    fn eq(&self, other: &UuidInfo) -> bool {
        self.uuid == other.uuid
    }
}

impl UuidInfo {
    pub fn process(&self, buffer: &[u8], s: &mut Sender<SensorValue>) {
        for (field, sensor) in self.fields.iter().zip(self.ha_values) {
            let value = field.decode::<f32>(buffer);
            let value = SensorValue::Float(*sensor, value);
            s.send(value).unwrap();
        }
    }

    pub fn byte_len(&self) -> usize {
        let last = self.fields.last().unwrap();
        let len = (last.offset + last.length + (8-1)) /8;
        len as usize
    }
}

pub const SENSORS: &'static [DeviceInfo] = &[
    DeviceInfo {
        adress: "0A:0A:0A:0A:0A:0A",
        values: &[
            UuidInfo {
                uuid: "93700001-1bb7-1599-985b-f5e7dc991483",
                fields: &[Field::<f32> {
                    decode_add: 1.,
                    decode_scale: 1.,
                    length: 1,
                    offset: 1,
                }],
                ha_values: &[Sensor::Humidity],
            }
        ]
    },
    /*DeviceInfo {
        adress: "0A:0A:0A:0A:0A:0A",
        values: &[
            UuidInfo {
                uuid: "93700002-1bb7-1599-985b-f5e7dc991483",
                fields: &[Field::<f32> {
                    decode_add: 1.,
                    decode_scale: 1.,
                    length: 1,
                    offset: 1,
                }],
                ha_values: &[Sensor::Humidity],
            }
        ]
    },*/
];