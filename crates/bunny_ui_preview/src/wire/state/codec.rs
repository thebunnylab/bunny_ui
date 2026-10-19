//! Additive bounded payloads; existing message layouts remain frozen.
use super::*;
use crate::wire::{Cursor, Malformed, Put};

fn text(c: &mut Cursor<'_>) -> Result<String, Malformed> {
    let len = c.u32()? as usize;
    if len > STATE_TEXT_LIMIT {
        return Err(Malformed::StateObservationLimit);
    }
    String::from_utf8(c.take(len)?.to_vec()).map_err(|_| Malformed::BadText)
}
impl StateRequestId {
    pub(in crate::wire) fn get_wire(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Self::parse(c.u64()?).map_err(|_| Malformed::InvalidStateObservation)
    }
}
impl StateTarget {
    pub(in crate::wire) fn put(&self, out: &mut Vec<u8>) {
        out.option(self.instance.as_ref(), |out, value| value.put(out));
        out.string(&self.preview);
        out.u32(self.variant);
        out.u32(self.generation);
        out.u64(self.mounted.get());
    }
    pub(in crate::wire) fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Self {
            instance: c.option(|c| {
                SceneInstanceId::parse(&text(c)?).map_err(|_| Malformed::InvalidSceneInstance)
            })?,
            preview: text(c)?,
            variant: c.u32()?,
            generation: c.u32()?,
            mounted: MountedViewId::parse(c.u64()?).map_err(|_| Malformed::InvalidMountedView)?,
        })
    }
}
impl StateReply {
    pub(in crate::wire) fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Snapshot(snapshot) => {
                out.u8(0);
                out.u64(snapshot.request.get());
                snapshot.target.put(out);
                out.u64(snapshot.seq);
                snapshot.read.put(out);
            }
            Self::Stopped { request } => {
                out.u8(1);
                out.u64(request.get());
            }
            Self::Refused { request, reason } => {
                out.u8(2);
                out.u64(request.get());
                reason.put(out);
            }
        }
    }
    pub(in crate::wire) fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        let tag = c.u8()?;
        let request = StateRequestId::get_wire(c)?;
        match tag {
            0 => Ok(Self::Snapshot(StateSnapshot {
                request,
                target: StateTarget::get(c)?,
                seq: c.u64()?,
                read: StateRead::get(c)?,
            })),
            1 => Ok(Self::Stopped { request }),
            2 => Ok(Self::Refused {
                request,
                reason: StateRefusal::get(c)?,
            }),
            tag => Err(Malformed::BadTag {
                what: "StateReply",
                tag,
            }),
        }
    }
}
impl StateRefusal {
    fn put(self, out: &mut Vec<u8>) {
        out.u8(match self {
            Self::Disabled => 0,
            Self::SceneMissing => 1,
            Self::AmbiguousScene => 2,
            Self::Generation => 3,
            Self::NotPublished => 4,
            Self::ViewMissing => 5,
            Self::Poisoned => 6,
            Self::StaleRequest => 7,
            Self::Limit => 8,
        });
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        match c.u8()? {
            0 => Ok(Self::Disabled),
            1 => Ok(Self::SceneMissing),
            2 => Ok(Self::AmbiguousScene),
            3 => Ok(Self::Generation),
            4 => Ok(Self::NotPublished),
            5 => Ok(Self::ViewMissing),
            6 => Ok(Self::Poisoned),
            7 => Ok(Self::StaleRequest),
            8 => Ok(Self::Limit),
            tag => Err(Malformed::BadTag {
                what: "StateRefusal",
                tag,
            }),
        }
    }
}
impl StateRead {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Values(deps) => {
                out.u8(0);
                out.list(deps, |out, dep| dep.put(out));
            }
            Self::NoBinding => out.u8(1),
            Self::UnavailableBinding => out.u8(2),
            Self::Borrowed => out.u8(3),
            Self::Limit => out.u8(4),
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        match c.u8()? {
            0 => {
                let count = c.u32()? as usize;
                if count > STATE_DEPENDENCY_LIMIT {
                    return Err(Malformed::StateObservationLimit);
                }
                // Bounds are checked before allocating, independently of frame size.
                let mut deps = Vec::with_capacity(count);
                for _ in 0..count {
                    deps.push(StateDependency::get(c)?);
                }
                let read = Self::Values(deps);
                read.validate().map_err(|reason| match reason {
                    StateMismatch::Limit => Malformed::StateObservationLimit,
                    _ => Malformed::InvalidStateObservation,
                })?;
                Ok(read)
            }
            1 => Ok(Self::NoBinding),
            2 => Ok(Self::UnavailableBinding),
            3 => Ok(Self::Borrowed),
            4 => Ok(Self::Limit),
            tag => Err(Malformed::BadTag {
                what: "StateRead",
                tag,
            }),
        }
    }
}
impl StateDependency {
    fn put(&self, out: &mut Vec<u8>) {
        match self.key {
            DependencyKey::State(id) => {
                out.u8(0);
                out.u64(id);
            }
            DependencyKey::Store(id) => {
                out.u8(1);
                out.u64(id);
            }
        }
        match &self.evidence {
            StateEvidence::State {
                type_name,
                constructor,
                version,
                value,
            } => {
                out.u8(0);
                out.string(type_name);
                out.string(&constructor.file);
                out.u32(constructor.line);
                out.u32(constructor.column);
                out.u64(*version);
                value.put(out);
            }
            StateEvidence::Missing => out.u8(1),
            StateEvidence::Unsupported => out.u8(2),
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        let key = match c.u8()? {
            0 => DependencyKey::State(c.u64()?),
            1 => DependencyKey::Store(c.u64()?),
            tag => {
                return Err(Malformed::BadTag {
                    what: "DependencyKey",
                    tag,
                });
            }
        };
        let evidence = match c.u8()? {
            0 => StateEvidence::State {
                type_name: text(c)?,
                constructor: StateConstructor {
                    file: text(c)?,
                    line: c.u32()?,
                    column: c.u32()?,
                },
                version: c.u64()?,
                value: StateValue::get(c)?,
            },
            1 => StateEvidence::Missing,
            2 => StateEvidence::Unsupported,
            tag => {
                return Err(Malformed::BadTag {
                    what: "StateEvidence",
                    tag,
                });
            }
        };
        Ok(Self { key, evidence })
    }
}
impl StateValue {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Signed(value) => {
                out.u8(0);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::Unsigned(value) => {
                out.u8(1);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::FloatBits(value) => {
                out.u8(2);
                out.u64(*value);
            }
            Self::Bool(value) => {
                out.u8(3);
                out.bool(*value);
            }
            Self::Char(value) => {
                out.u8(4);
                out.u32(u32::from(*value));
            }
            Self::Text(value) => {
                out.u8(5);
                out.string(value);
            }
            Self::Opaque => out.u8(6),
            Self::TooLarge { bytes } => {
                out.u8(7);
                out.u64(*bytes);
            }
            Self::Updating => out.u8(8),
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        match c.u8()? {
            0 => {
                let mut bytes = [0; 16];
                bytes.copy_from_slice(c.take(16)?);
                Ok(Self::Signed(i128::from_le_bytes(bytes)))
            }
            1 => {
                let mut bytes = [0; 16];
                bytes.copy_from_slice(c.take(16)?);
                Ok(Self::Unsigned(u128::from_le_bytes(bytes)))
            }
            2 => Ok(Self::FloatBits(c.u64()?)),
            3 => Ok(Self::Bool(c.bool()?)),
            4 => char::from_u32(c.u32()?)
                .map(Self::Char)
                .ok_or(Malformed::InvalidStateObservation),
            5 => Ok(Self::Text(text(c)?)),
            6 => Ok(Self::Opaque),
            7 => Ok(Self::TooLarge { bytes: c.u64()? }),
            8 => Ok(Self::Updating),
            tag => Err(Malformed::BadTag {
                what: "StateValue",
                tag,
            }),
        }
    }
}
