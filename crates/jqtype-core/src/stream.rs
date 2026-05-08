use serde::{Deserialize, Serialize};

use crate::types::JType;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cardinality {
    Zero,
    One,
    ZeroOrOne,
    OneOrMore,
    ZeroOrMore,
}

impl Cardinality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Cardinality::Zero => "zero",
            Cardinality::One => "one",
            Cardinality::ZeroOrOne => "zero_or_one",
            Cardinality::OneOrMore => "one_or_more",
            Cardinality::ZeroOrMore => "zero_or_more",
        }
    }

    pub fn join(self, other: Self) -> Self {
        use Cardinality::*;
        match (self, other) {
            (Zero, x) | (x, Zero) => x,
            (One, One) => OneOrMore,
            (One, ZeroOrOne) | (ZeroOrOne, One) | (ZeroOrOne, ZeroOrOne) => ZeroOrMore,
            (OneOrMore, _) | (_, OneOrMore) | (ZeroOrMore, _) | (_, ZeroOrMore) => ZeroOrMore,
        }
    }

    pub fn alternative(self, other: Self) -> Self {
        use Cardinality::*;
        match (self, other) {
            (Zero, Zero) => Zero,
            (Zero, One) | (One, Zero) | (Zero, ZeroOrOne) | (ZeroOrOne, Zero) => ZeroOrOne,
            (Zero, OneOrMore) | (OneOrMore, Zero) => ZeroOrMore,
            (Zero, ZeroOrMore) | (ZeroOrMore, Zero) => ZeroOrMore,
            (One, One) => One,
            (One, ZeroOrOne) | (ZeroOrOne, One) | (ZeroOrOne, ZeroOrOne) => ZeroOrOne,
            (One, OneOrMore) | (OneOrMore, One) | (OneOrMore, OneOrMore) => OneOrMore,
            (One, ZeroOrMore)
            | (ZeroOrMore, One)
            | (ZeroOrOne, OneOrMore)
            | (OneOrMore, ZeroOrOne)
            | (ZeroOrOne, ZeroOrMore)
            | (ZeroOrMore, ZeroOrOne)
            | (OneOrMore, ZeroOrMore)
            | (ZeroOrMore, OneOrMore)
            | (ZeroOrMore, ZeroOrMore) => ZeroOrMore,
        }
    }

    /// Returns `true` when `count` is a valid number of values for this
    /// cardinality (e.g. `One` accepts exactly one, `OneOrMore` accepts any
    /// non-zero count).
    pub fn fits_count(&self, count: usize) -> bool {
        use Cardinality::*;
        match self {
            Zero => count == 0,
            One => count == 1,
            ZeroOrOne => count <= 1,
            OneOrMore => count >= 1,
            ZeroOrMore => true,
        }
    }

    pub fn compose(self, inner: Self) -> Self {
        use Cardinality::*;
        match (self, inner) {
            (Zero, _) | (_, Zero) => Zero,
            (One, c) => c,
            (ZeroOrOne, One) => ZeroOrOne,
            (ZeroOrOne, ZeroOrOne) => ZeroOrOne,
            (ZeroOrOne, OneOrMore | ZeroOrMore) => ZeroOrMore,
            (OneOrMore, One) => OneOrMore,
            (OneOrMore, ZeroOrOne | ZeroOrMore) => ZeroOrMore,
            (OneOrMore, OneOrMore) => OneOrMore,
            (ZeroOrMore, _) => ZeroOrMore,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamType {
    pub item: JType,
    pub card: Cardinality,
}

impl StreamType {
    pub fn new(item: JType, card: Cardinality) -> Self {
        if matches!(card, Cardinality::Zero) {
            Self {
                item: JType::Never,
                card,
            }
        } else {
            Self { item, card }
        }
    }

    pub fn one(item: JType) -> Self {
        Self::new(item, Cardinality::One)
    }

    pub fn zero() -> Self {
        Self::new(JType::Never, Cardinality::Zero)
    }

    pub fn zero_or_one(item: JType) -> Self {
        Self::new(item, Cardinality::ZeroOrOne)
    }

    pub fn zero_or_more(item: JType) -> Self {
        Self::new(item, Cardinality::ZeroOrMore)
    }

    pub fn join(self, other: Self) -> Self {
        Self::new(
            JType::union([self.item, other.item]),
            self.card.join(other.card),
        )
    }

    pub fn join_alternative(self, other: Self) -> Self {
        Self::new(
            JType::union([self.item, other.item]),
            self.card.alternative(other.card),
        )
    }

    pub fn to_compact_string(&self) -> String {
        if matches!(self.card, Cardinality::One) {
            self.item.to_compact_string()
        } else {
            format!("Stream<{}, {:?}>", self.item.to_compact_string(), self.card)
        }
    }

    /// Returns `true` when `outputs` is a possible concrete output of a
    /// filter with this stream type. Used by the compatibility harness.
    pub fn fits_outputs(
        &self,
        outputs: &[serde_json::Value],
        item_check: impl Fn(&serde_json::Value, &JType) -> bool,
    ) -> bool {
        if !self.card.fits_count(outputs.len()) {
            return false;
        }
        outputs.iter().all(|value| item_check(value, &self.item))
    }
}
