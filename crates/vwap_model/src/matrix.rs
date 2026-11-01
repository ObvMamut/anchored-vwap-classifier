/// Class probabilities per row, in the fixed class order (rejection, breakthrough, chop).
pub type Probs = Vec<[f64; 3]>;

/// Row major matrix of `f64`. Missing values are NaN.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub n: usize,
    pub p: usize,
    pub data: Vec<f64>,
}

impl Matrix {
    pub fn new(n: usize, p: usize, data: Vec<f64>) -> Option<Self> {
        (data.len() == n * p).then_some(Self { n, p, data })
    }

    pub fn from_rows(rows: &[Vec<f64>]) -> Self {
        let p = rows.first().map_or(0, Vec::len);
        let mut data = Vec::with_capacity(rows.len() * p);
        for r in rows {
            for j in 0..p {
                data.push(r.get(j).copied().unwrap_or(f64::NAN));
            }
        }
        Self {
            n: rows.len(),
            p,
            data,
        }
    }

    pub fn row(&self, i: usize) -> &[f64] {
        let a = i * self.p;
        self.data.get(a..a + self.p).unwrap_or(&[])
    }

    pub fn get(&self, i: usize, j: usize) -> f64 {
        self.data.get(i * self.p + j).copied().unwrap_or(f64::NAN)
    }

    pub fn column(&self, j: usize) -> Vec<f64> {
        (0..self.n).map(|i| self.get(i, j)).collect()
    }

    /// The listed rows, in the listed order.
    pub fn select(&self, idx: &[usize]) -> Self {
        let mut data = Vec::with_capacity(idx.len() * self.p);
        for &i in idx {
            data.extend_from_slice(self.row(i));
        }
        Self {
            n: idx.len(),
            p: self.p,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_keeps_order_and_rows() {
        let m = Matrix::from_rows(&[vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 6.0]]);
        let s = m.select(&[2, 0]);
        assert_eq!(s.n, 2);
        assert_eq!(s.row(0), &[5.0, 6.0]);
        assert_eq!(s.row(1), &[1.0, 2.0]);
        assert_eq!(m.column(1), vec![2.0, 4.0, 6.0]);
        assert!(Matrix::new(2, 2, vec![1.0]).is_none());
    }
}
