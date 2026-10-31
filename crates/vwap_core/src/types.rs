//! Small enums shared by every stage.

use serde::{Deserialize, Serialize};

/// Bar width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    M1,
    M5,
    M15,
}

impl Timeframe {
    pub fn minutes(self) -> u32 {
        match self {
            Timeframe::M1 => 1,
            Timeframe::M5 => 5,
            Timeframe::M15 => 15,
        }
    }
}

/// Approach side: `Above` means price came from above the VWAP, so s = +1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Above,
    Below,
}

impl Side {
    pub fn sign(self) -> i8 {
        match self {
            Side::Above => 1,
            Side::Below => -1,
        }
    }
}

/// Outcome class. The index order 0, 1, 2 is used by every model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Class {
    Rejection,
    Breakthrough,
    Chop,
}

impl Class {
    pub const ALL: [Class; 3] = [Class::Rejection, Class::Breakthrough, Class::Chop];

    pub fn index(self) -> usize {
        match self {
            Class::Rejection => 0,
            Class::Breakthrough => 1,
            Class::Chop => 2,
        }
    }
}
