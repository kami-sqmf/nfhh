//! 極簡固定視窗限流，給沒有登入的公開端點用（`/api/join/start`）。
//! 記憶體內、重啟歸零 —— 這裡要擋的是自動化灌入，不是精準計費。

use std::collections::HashMap;
use std::sync::Mutex;

pub struct Limiter {
    window_secs: i64,
    per_key: u32,
    global: u32,
    inner: Mutex<State>,
}

struct State {
    window_start: i64,
    total: u32,
    per_key: HashMap<String, u32>,
}

impl Limiter {
    pub fn new(window_secs: i64, per_key: u32, global: u32) -> Self {
        Self {
            window_secs,
            per_key,
            global,
            inner: Mutex::new(State {
                window_start: 0,
                total: 0,
                per_key: HashMap::new(),
            }),
        }
    }

    /// 回 true = 放行並計數。視窗到期時整組歸零，所以 HashMap 不會無限長。
    pub fn allow(&self, key: &str, now: i64) -> bool {
        let mut s = self.inner.lock().unwrap();
        // 時鐘往回跳（NTP 校正）也算視窗結束。只用 `>=` 判斷的話差值是負數，
        // 視窗永遠不會重開，限流器就卡在滿的狀態把正常人一起關在門外。
        if now < s.window_start || now - s.window_start >= self.window_secs {
            s.window_start = now;
            s.total = 0;
            s.per_key.clear();
        }
        if s.total >= self.global {
            return false;
        }
        let n = s.per_key.entry(key.to_string()).or_insert(0);
        if *n >= self.per_key {
            return false;
        }
        *n += 1;
        s.total += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_key_limit_then_window_reset() {
        let l = Limiter::new(60, 2, 100);
        assert!(l.allow("a", 0));
        assert!(l.allow("a", 1));
        assert!(!l.allow("a", 2), "第三次要擋");
        assert!(l.allow("b", 2), "別的 key 不受影響");
        assert!(l.allow("a", 60), "視窗過了要放行");
    }

    /// NTP 把時鐘往回撥時，`now - window_start` 是負數 —— 用 `>=` 判斷到期
    /// 的話視窗永遠不會重開，限流器就卡在滿的狀態，把正常人一起關在門外。
    #[test]
    fn a_backwards_clock_step_does_not_freeze_the_limiter() {
        let l = Limiter::new(60, 1, 100);
        assert!(l.allow("a", 1_000));
        assert!(!l.allow("a", 1_001));
        assert!(l.allow("a", 500), "時鐘倒退等於換了一個視窗");
    }

    #[test]
    fn global_limit_caps_everyone() {
        let l = Limiter::new(60, 10, 3);
        assert!(l.allow("a", 0));
        assert!(l.allow("b", 0));
        assert!(l.allow("c", 0));
        assert!(!l.allow("d", 0));
    }
}
