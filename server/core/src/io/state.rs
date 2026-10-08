//! The process image: the current value of every I/O point. Inputs are set from outside
//! the PLC (by the host, e.g. from the IDE's I/O panel); outputs are written by the PLC
//! after each scan.

use serde::{Deserialize, Serialize};

use super::{IoConfig, IoKind};
use crate::compiler::value::Value;

/// One I/O point and its current value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IoPoint {
    pub address: String,
    pub kind: IoKind,
    pub value: Value,
}

/// The process image. Inputs belong to the outside world, outputs to the PLC.
#[derive(Debug, Clone)]
pub struct IoImage {
    pub(super) points: Vec<IoPoint>,
}

impl IoImage {
    pub fn new(config: &IoConfig) -> Self {
        let points = config
            .points()
            .into_iter()
            .map(|(address, kind)| IoPoint { address, kind, value: Value::default_for(kind.data_type()) })
            .collect();
        Self { points }
    }

    pub fn points(&self) -> &[IoPoint] {
        &self.points
    }

    /// Point `index` of `kind`.
    pub(super) fn point_mut(&mut self, kind: IoKind, index: usize) -> Option<&mut IoPoint> {
        let address = kind.address(index);
        self.points.iter_mut().find(|p| p.address == address)
    }

    pub(super) fn point(&self, kind: IoKind, index: usize) -> Option<&IoPoint> {
        let address = kind.address(index);
        self.points.iter().find(|p| p.address == address)
    }

    /// The safe state when the PLC isn't running: every output OFF / 0.0. Inputs are kept.
    pub fn reset_outputs(&mut self) {
        for point in self.points.iter_mut().filter(|p| !p.kind.is_input()) {
            point.value = Value::default_for(point.kind.data_type());
        }
    }
}
