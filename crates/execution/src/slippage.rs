use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use cripton_core::Side;

const MAX_TOLERANCE_PCT: Decimal = dec!(5.0); // 5% max slippage — anything higher is suspicious

/// Calculate the maximum acceptable price considering slippage tolerance.
///
/// For buys: max_price = signal_price * (1 + tolerance)
/// For sells: min_price = signal_price * (1 - tolerance)
///
/// SEC: tolerance_pct is clamped to [0, MAX_TOLERANCE_PCT]
pub fn apply_slippage(price: Decimal, side: Side, tolerance_pct: Decimal) -> Decimal {
    // SEC: clamp tolerance to safe bounds
    let clamped = tolerance_pct.max(Decimal::ZERO).min(MAX_TOLERANCE_PCT);
    let tolerance = clamped / dec!(100);

    match side {
        Side::Buy => price * (Decimal::ONE + tolerance),
        Side::Sell => price * (Decimal::ONE - tolerance),
    }
}

/// Check if the current market price is within acceptable slippage.
pub fn is_within_slippage(
    signal_price: Decimal,
    market_price: Decimal,
    side: Side,
    tolerance_pct: Decimal,
) -> bool {
    let limit = apply_slippage(signal_price, side, tolerance_pct);
    match side {
        Side::Buy => market_price <= limit,
        Side::Sell => market_price >= limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buy_slippage() {
        let price = dec!(1.0000);
        let result = apply_slippage(price, Side::Buy, dec!(0.05));
        assert_eq!(result, dec!(1.00050000));
    }

    #[test]
    fn test_sell_slippage() {
        let price = dec!(1.0000);
        let result = apply_slippage(price, Side::Sell, dec!(0.05));
        assert_eq!(result, dec!(0.99950000));
    }

    #[test]
    fn test_within_slippage_buy() {
        assert!(is_within_slippage(dec!(1.0), dec!(1.0004), Side::Buy, dec!(0.05)));
        assert!(!is_within_slippage(dec!(1.0), dec!(1.001), Side::Buy, dec!(0.05)));
    }

    #[test]
    fn test_negative_tolerance_clamped_to_zero() {
        let price = dec!(1.0000);
        let result = apply_slippage(price, Side::Buy, dec!(-10));
        assert_eq!(result, dec!(1.0000)); // no slippage applied
    }

    #[test]
    fn test_extreme_tolerance_clamped() {
        let price = dec!(1.0000);
        let result = apply_slippage(price, Side::Buy, dec!(999));
        // Clamped to 5%, so result = 1.05
        assert_eq!(result, dec!(1.05000));
    }
}
