use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use cripton_core::Side;

/// Calculate the maximum acceptable price considering slippage tolerance.
///
/// For buys: max_price = signal_price * (1 + tolerance)
/// For sells: min_price = signal_price * (1 - tolerance)
pub fn apply_slippage(price: Decimal, side: Side, tolerance_pct: Decimal) -> Decimal {
    let tolerance = tolerance_pct / dec!(100);
    match side {
        Side::Buy => price * (Decimal::ONE + tolerance),
        Side::Sell => price * (Decimal::ONE - tolerance),
    }
}

/// Check if the current market price is within acceptable slippage.
///
/// For buys: acceptable if market_price <= signal_price * (1 + tolerance)
/// For sells: acceptable if market_price >= signal_price * (1 - tolerance)
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
        let result = apply_slippage(price, Side::Buy, dec!(0.05)); // 0.05%
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
}
