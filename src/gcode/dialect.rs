//! The GCode dialect's **typed-value surface**: what a template may *do* with a
//! `Length`, `FeedRate`, `RotationalSpeed` or `Angle` once it has one.
//!
//! `docs/gcode-engine.md` §4 and `docs/gcode-template-language.md` §6 specify it —
//! comparisons, arithmetic, `max`/`min`/`abs`/`clamp`, and `.mm`-style accessors — so
//! that script logic reads in the quantities the machinist thinks in rather than in
//! bare numbers whose unit the author has to track:
//!
//! ```text
//! let z = z_retract;
//! while z > z_bottom {
//!     z = max(z - peck, z_bottom);
//!     `G1 Z{z} F{z_feedrate}
//! }
//! ```
//!
//! # Why this is registered rather than implemented on the types
//!
//! The `units` types derive a **structural** `PartialEq` over `(scalar, unit)`, so in
//! Rust `Length::from_mm(10.0) != Length::from_cm(1.0)` — the same length written two
//! ways. That is the right answer for "did the operator edit this field", which is what
//! the app uses equality for (profile dirty-checking, Dioxus props, board snapshots).
//! It is the wrong answer for a script asking whether one depth is past another.
//!
//! Deriving `PartialOrd` beside it would be worse than either: `partial_cmp` would say
//! `Equal` where `==` says `false`, breaking the consistency the two traits are required
//! to keep. So the script-facing meaning lives here, on the engine, where it cannot
//! reach the rest of the application — which is also what `docs/gcode-engine.md` §8 says
//! the Coder is for.
//!
//! # Comparison is canonical, with a tolerance
//!
//! Every operator goes through [`Unit::canonical`], so a value is compared as the
//! quantity it *is*, not as it was written. [`Unit::EPS`] sits a thousandfold below
//! emitted precision (1 nm for a length rounded to the micron) and exists for loop
//! termination: `while z > z_bottom` must stop when the two are the same depth, not
//! spin on a last-bit difference left by a unit conversion. Equality within a tolerance
//! is not transitive; at this scale nothing reachable from a machining template can
//! observe that.
//!
//! # A mismatch must raise, because Rhai will not
//!
//! Rhai answers a comparison between two *different* non-numeric types with a constant:
//! `false` for `== < <= > >=`, `true` for `!=` (`func/builtin.rs`, "Default comparison
//! operators for different, non-numeric types"). No error, no diagnostic. So `z > 5` —
//! five what? — would quietly be `false`, and a `while z > z_bottom` whose bound is
//! accidentally a bare number becomes a loop that never runs: the step emits no cutting
//! moves and the board comes out undrilled behind a program that reads perfectly well.
//!
//! Leaving those combinations unregistered is therefore not the neutral choice it looks
//! like. Each one is registered to fail (see [`reject_lhs`]), which is the only way to
//! get an error instead of a wrong answer.

use std::cell::Cell;
use std::cmp::Ordering;
use std::rc::Rc;

use gtl::rhai::{Dynamic, Engine, EvalAltResult, NativeCallContext, Position};
use units::{Angle, FeedRate, Length, RotationalSpeed, UserUnitSystem};

/// The per-type facts the generic registration needs: how to reach the quantity as a
/// single canonical number, how to build one back, and how it is spelled.
///
/// One trait rather than a macro because every operator is then written once, against
/// `canonical`, which is what makes `<`, `==` and `>=` incapable of disagreeing.
///
/// The supertraits are what Rhai's own `Variant` blanket impl needs from a registered
/// type; `Variant` itself is only nameable under Rhai's `internals` feature.
trait Unit: Copy + Send + Sync + 'static {
    /// What templates see in an error and from `type_of()`. Without it a mismatch
    /// reports `units::types::Length`, a Rust path no profile author should meet.
    const NAME: &'static str;

    /// The accessor named in the mismatch message, as the way out for someone who
    /// really did mean to compare against a plain number.
    const ACCESSOR: &'static str;

    /// Tie tolerance, in canonical units. See the module docs.
    const EPS: f64;

    /// The quantity as one number, in this type's canonical unit.
    fn canonical(self) -> f64;

    /// Rebuilds a value from a canonical number — the carrier for arithmetic results.
    fn from_canonical(value: f64) -> Self;

    /// The bare machine number for emission, in the active unit system.
    fn format(self, system: UserUnitSystem) -> String;
}

impl Unit for Length {
    const NAME: &'static str = "Length";
    const ACCESSOR: &'static str = "mm";
    /// 1 nm, against coordinates emitted to the micron.
    const EPS: f64 = 1e-6;

    fn canonical(self) -> f64 {
        self.as_mm()
    }

    /// Millimetres, not nanometres: `Length::from_nm` takes an `i64`, and a saturating
    /// `f64 → i64` cast would turn an overflowing expression into a plausible-looking
    /// coordinate at the end of the machine's travel rather than an error.
    fn from_canonical(value: f64) -> Self {
        Length::from_mm(value)
    }

    fn format(self, system: UserUnitSystem) -> String {
        units::machine::number_length(self, system)
    }
}

impl Unit for FeedRate {
    const NAME: &'static str = "FeedRate";
    const ACCESSOR: &'static str = "mm_per_min";
    const EPS: f64 = 1e-6;

    fn canonical(self) -> f64 {
        self.as_mm_per_min()
    }

    fn from_canonical(value: f64) -> Self {
        FeedRate::from_mm_per_min(value)
    }

    fn format(self, system: UserUnitSystem) -> String {
        units::machine::number_feed(self, system)
    }
}

impl Unit for RotationalSpeed {
    const NAME: &'static str = "RotationalSpeed";
    const ACCESSOR: &'static str = "rpm";
    const EPS: f64 = 1e-6;

    fn canonical(self) -> f64 {
        self.as_rpm()
    }

    fn from_canonical(value: f64) -> Self {
        RotationalSpeed::from_rpm(value)
    }

    fn format(self, system: UserUnitSystem) -> String {
        units::machine::number_speed(self, system)
    }
}

impl Unit for Angle {
    const NAME: &'static str = "Angle";
    const ACCESSOR: &'static str = "degrees";
    /// Tighter than the others: a degree is a coarse unit, so a 1e-6 tie would swallow
    /// a difference a template might legitimately branch on.
    const EPS: f64 = 1e-9;

    fn canonical(self) -> f64 {
        self.as_degrees()
    }

    fn from_canonical(value: f64) -> Self {
        Angle::from_degrees(value)
    }

    fn format(self, system: UserUnitSystem) -> String {
        units::machine::number_angle(self, system)
    }
}

/// The single ordering every comparison derives from, so they cannot disagree.
///
/// NaN sorts as `Greater` rather than panicking; it can only arise from a division this
/// module already rejects, and a template is not the place to discover a poisoned float.
fn ord<T: Unit>(a: T, b: T) -> Ordering {
    let (a, b) = (a.canonical(), b.canonical());
    if (a - b).abs() <= T::EPS {
        Ordering::Equal
    } else if a < b {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// The comparison operators, all of which must be registered explicitly — Rhai
/// synthesises none of them from the others (`packages/logic.rs` registers all six by
/// hand for its own types).
const COMPARISONS: [&str; 6] = ["==", "!=", "<", "<=", ">", ">="];

/// The error a mismatched comparison raises.
///
/// `ErrorMismatchDataType` rather than a thrown string: a string arrives as
/// `ErrorRuntime`, which `gtl` maps to `GtlError::Thrown` — and that variant carries no
/// line number, so the author would be told *what* is wrong without being told *where*.
fn mismatch<T: Unit>(ctx: &NativeCallContext, other: &Dynamic) -> Box<EvalAltResult> {
    EvalAltResult::ErrorMismatchDataType(
        format!(
            "{name} — compare it with another {name}, or take a plain number from it \
             with an accessor such as .{accessor}",
            name = T::NAME,
            accessor = T::ACCESSOR,
        ),
        ctx.engine().map_type_name(other.type_name()).to_string(),
        Position::NONE,
    )
    .into()
}

/// `<unit> <op> <anything else>`.
///
/// A free `fn` rather than a closure: the registration bound is
/// `for<'a> Fn(NativeCallContext<'a>, ..)`, and a closure's inferred lifetime does not
/// satisfy that higher-ranked bound.
fn reject_lhs<T: Unit>(
    ctx: NativeCallContext,
    _lhs: T,
    rhs: Dynamic,
) -> Result<bool, Box<EvalAltResult>> {
    Err(mismatch::<T>(&ctx, &rhs))
}

/// `<anything else> <op> <unit>`. Both orders are needed: Rhai's dispatch is positional.
fn reject_rhs<T: Unit>(
    ctx: NativeCallContext,
    lhs: Dynamic,
    _rhs: T,
) -> Result<bool, Box<EvalAltResult>> {
    Err(mismatch::<T>(&ctx, &lhs))
}

/// Registers the whole surface for one unit type.
fn register_unit<T: Unit>(engine: &mut Engine, mode: &Rc<Cell<UserUnitSystem>>) {
    engine.register_type_with_name::<T>(T::NAME);

    let system = mode.clone();
    engine.register_fn("fmt", move |value: T| value.format(system.get()));

    // --- comparison -------------------------------------------------------------
    engine.register_fn("==", |a: T, b: T| ord(a, b) == Ordering::Equal);
    engine.register_fn("!=", |a: T, b: T| ord(a, b) != Ordering::Equal);
    engine.register_fn("<", |a: T, b: T| ord(a, b) == Ordering::Less);
    engine.register_fn("<=", |a: T, b: T| ord(a, b) != Ordering::Greater);
    engine.register_fn(">", |a: T, b: T| ord(a, b) == Ordering::Greater);
    engine.register_fn(">=", |a: T, b: T| ord(a, b) != Ordering::Less);

    // A comparison against anything else is an error, not `false`. See the module docs.
    for op in COMPARISONS {
        engine.register_fn(op, reject_lhs::<T>);
        engine.register_fn(op, reject_rhs::<T>);
    }

    // --- arithmetic -------------------------------------------------------------
    // `+=` and `-=` need no registration: Rhai rewrites a missing op-assignment to
    // `var = var <op> rhs` and finds these.
    engine.register_fn("+", |a: T, b: T| T::from_canonical(a.canonical() + b.canonical()));
    engine.register_fn("-", |a: T, b: T| T::from_canonical(a.canonical() - b.canonical()));
    engine.register_fn("-", |a: T| T::from_canonical(-a.canonical()));

    // Both integer and float scaling, in both orders: Rhai does not coerce numeric
    // types across a registered signature, so `z * 2` and `z * 2.0` are distinct calls,
    // and `2 * z` is a different call again.
    engine.register_fn("*", |a: T, n: i64| T::from_canonical(a.canonical() * n as f64));
    engine.register_fn("*", |a: T, n: f64| T::from_canonical(a.canonical() * n));
    engine.register_fn("*", |n: i64, a: T| T::from_canonical(a.canonical() * n as f64));
    engine.register_fn("*", |n: f64, a: T| T::from_canonical(a.canonical() * n));

    // Division guards zero rather than yielding an infinity: `Length::from_mm(inf)`
    // formats as "inf", which would reach the controller as a coordinate.
    engine.register_fn("/", |a: T, n: i64| divide(a, n as f64));
    engine.register_fn("/", |a: T, n: f64| divide(a, n));
    engine.register_fn("/", |a: T, b: T| ratio(a, b));

    // --- helpers ----------------------------------------------------------------
    // These return one of their *arguments*, never a rebuilt value. `z = max(z - peck,
    // z_bottom)` must hand back the identical `z_bottom`, or the round trip through the
    // canonical carrier shifts its last bit and the loop takes one more pass than the
    // author wrote, emitting a duplicate move.
    engine.register_fn("max", |a: T, b: T| if ord(a, b) == Ordering::Less { b } else { a });
    engine.register_fn("min", |a: T, b: T| if ord(a, b) == Ordering::Greater { b } else { a });
    engine.register_fn("abs", |a: T| {
        if a.canonical() < 0.0 {
            T::from_canonical(-a.canonical())
        } else {
            a
        }
    });
    engine.register_fn("clamp", |value: T, low: T, high: T| clamp(value, low, high));
}

/// `<unit> / number`, rejecting a zero divisor.
fn divide<T: Unit>(a: T, divisor: f64) -> Result<T, Box<EvalAltResult>> {
    if divisor == 0.0 {
        return Err(zero_divisor::<T>());
    }
    Ok(T::from_canonical(a.canonical() / divisor))
}

/// `<unit> / <unit>` — a plain ratio, for pass counts and stepovers.
fn ratio<T: Unit>(a: T, b: T) -> Result<f64, Box<EvalAltResult>> {
    if b.canonical() == 0.0 {
        return Err(zero_divisor::<T>());
    }
    Ok(a.canonical() / b.canonical())
}

fn zero_divisor<T: Unit>() -> Box<EvalAltResult> {
    EvalAltResult::ErrorArithmetic(
        format!("division of a {} by zero", T::NAME),
        Position::NONE,
    )
    .into()
}

/// Clamps `value` to `low..=high`, returning whichever bound (or `value`) applies
/// unchanged. An inverted range is the author's mistake, not something to silently
/// reinterpret.
fn clamp<T: Unit>(value: T, low: T, high: T) -> Result<T, Box<EvalAltResult>> {
    if ord(low, high) == Ordering::Greater {
        return Err(EvalAltResult::ErrorArithmetic(
            format!("clamp was given a low bound above its high bound ({} values)", T::NAME),
            Position::NONE,
        )
        .into());
    }
    Ok(match () {
        _ if ord(value, low) == Ordering::Less => low,
        _ if ord(value, high) == Ordering::Greater => high,
        _ => value,
    })
}

/// Registers the whole typed-value surface on `engine`.
///
/// `mode` is the Coder's active unit system, shared with `metric()`/`imperial()`, so a
/// value formats in whatever unit the program last selected.
pub(crate) fn register(engine: &mut Engine, mode: &Rc<Cell<UserUnitSystem>>) {
    register_unit::<Length>(engine, mode);
    register_unit::<FeedRate>(engine, mode);
    register_unit::<RotationalSpeed>(engine, mode);
    register_unit::<Angle>(engine, mode);

    // Accessors — the escape hatch to a plain number, and the only way to force a
    // specific unit regardless of the modal `G20`/`G21` state. `.inch` rather than
    // `.in`, which is a Rhai keyword.
    engine.register_get("mm", |v: &mut Length| v.as_mm());
    engine.register_get("cm", |v: &mut Length| v.as_cm());
    engine.register_get("inch", |v: &mut Length| v.as_inch());
    engine.register_get("mil", |v: &mut Length| v.as_mil());
    engine.register_get("mm_per_min", |v: &mut FeedRate| v.as_mm_per_min());
    engine.register_get("in_per_min", |v: &mut FeedRate| v.as_in_per_min());
    engine.register_get("rpm", |v: &mut RotationalSpeed| v.as_rpm());
    engine.register_get("degrees", |v: &mut Angle| v.as_degrees());
    engine.register_get("radians", |v: &mut Angle| v.as_radians());

    // Rhai ships `min`/`max`/`abs` for plain numbers but no `clamp` at all, and the
    // language docs promise it "over numbers and unit types".
    engine.register_fn("clamp", |value: i64, low: i64, high: i64| {
        clamp_number(value as f64, low as f64, high as f64).map(|v| v as i64)
    });
    engine.register_fn("clamp", |value: f64, low: f64, high: f64| {
        clamp_number(value, low, high)
    });
}

fn clamp_number(value: f64, low: f64, high: f64) -> Result<f64, Box<EvalAltResult>> {
    if low > high {
        return Err(EvalAltResult::ErrorArithmetic(
            "clamp was given a low bound above its high bound".to_string(),
            Position::NONE,
        )
        .into());
    }
    Ok(value.clamp(low, high))
}
