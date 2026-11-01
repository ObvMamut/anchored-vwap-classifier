//! Moments and the Sharpe ratio, per period (not annualised).

pub fn mean(x: &[f64]) -> f64 {
    if x.is_empty() {
        f64::NAN
    } else {
        x.iter().sum::<f64>() / x.len() as f64
    }
}

/// Sample standard deviation (divisor $n - 1$).
pub fn sample_sd(x: &[f64]) -> f64 {
    let n = x.len();
    if n < 2 {
        return f64::NAN;
    }
    let m = mean(x);
    (x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (n as f64 - 1.0)).sqrt()
}

/// Mean over sample standard deviation; zero when the series is constant.
pub fn sharpe(x: &[f64]) -> f64 {
    let sd = sample_sd(x);
    if sd.is_nan() || sd == 0.0 {
        0.0
    } else {
        mean(x) / sd
    }
}

/// Sample skewness $m_3 / m_2^{3/2}$ with moments divided by $n$.
pub fn skewness(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    if x.len() < 3 {
        return f64::NAN;
    }
    let m = mean(x);
    let m2 = x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n;
    let m3 = x.iter().map(|v| (v - m).powi(3)).sum::<f64>() / n;
    if m2 == 0.0 { 0.0 } else { m3 / m2.powf(1.5) }
}

/// Kurtosis $m_4 / m_2^2$, not excess: a normal distribution gives 3.
pub fn kurtosis(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    if x.len() < 4 {
        return f64::NAN;
    }
    let m = mean(x);
    let m2 = x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n;
    let m4 = x.iter().map(|v| (v - m).powi(4)).sum::<f64>() / n;
    if m2 == 0.0 { 3.0 } else { m4 / (m2 * m2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_match_hand_computations() {
        let x = [1.0, 2.0, 3.0, 4.0, 10.0];
        assert!((mean(&x) - 4.0).abs() < 1e-12);
        // deviations: -3 -2 -1 0 6 ; squares sum 50 ; sample variance 12.5
        assert!((sample_sd(&x) - 12.5f64.sqrt()).abs() < 1e-12);
        assert!((sharpe(&x) - 4.0 / 12.5f64.sqrt()).abs() < 1e-12);
        // m2 = 10, m3 = (-27 - 8 - 1 + 0 + 216) / 5 = 36, skew = 36 / 10^1.5
        assert!((skewness(&x) - 36.0 / 10.0f64.powf(1.5)).abs() < 1e-12);
        // m4 = (81 + 16 + 1 + 0 + 1296) / 5 = 278.8, kurtosis = 278.8 / 100
        assert!((kurtosis(&x) - 2.788).abs() < 1e-12);
    }

    #[test]
    fn constant_and_short_series_are_handled() {
        assert_eq!(sharpe(&[2.0, 2.0, 2.0]), 0.0);
        assert!(sample_sd(&[1.0]).is_nan());
        assert!(skewness(&[1.0, 2.0]).is_nan());
        assert_eq!(kurtosis(&[1.0; 5]), 3.0);
    }
}
