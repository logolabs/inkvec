//! Batch research interface: one whitespace-separated list of lower,upper pairs
//! per line. Invalid pairs block acceptance. No tracing or model estimation here.
use inkvec_fit::pareto::refinement::{accepts, Delta};
use std::io::{self, BufRead};

fn main() -> io::Result<()> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        let deltas: Vec<_> = line
            .split_whitespace()
            .map(|pair| {
                let (lower, upper) = pair.split_once(',')?;
                Some(Delta {
                    lower: lower.parse().ok()?,
                    upper: upper.parse().ok()?,
                })
            })
            .collect();
        println!("{}", u8::from(accepts(&deltas)));
    }
    Ok(())
}
