//! 有上限、會掃過期的記憶體 session store（第二輪審查報告 #1）。
//!
//! tower-sessions 附的 `MemoryStore` 只會在 `load` 時**過濾**過期紀錄，
//! 從不刪除；預設壽命兩週。匿名的登入／加入起手每一次都在 store 留一份
//! 紀錄（挑戰、信箱證明），攻擊者不必有帳號就能讓它一直長 —— 限流之後
//! 理論上限是每 10 分鐘 200 筆，但仍是兩週不釋放。這裡補上三件事：
//!
//! 1. **硬上限**：滿了先清所有已過期的；還是滿就踢 `expiry_date` 最早的。
//!    在限流之下，被踢的只可能是灌進來的匿名紀錄本身 —— 家人的登入 session
//!    壽命是兩週，永遠排在 15 分鐘的匿名紀錄後面。
//! 2. `load` 碰到過期紀錄順手刪掉，不只是過濾。
//! 3. `sweep`：背景工作定期呼叫，把沒人再碰的過期紀錄清掉。
//!
//! 沒有用 `#[async_trait]`：async-trait 不是這個 crate 的直接相依，
//! tower-sessions 也沒有重新匯出它。四個方法都不需要在鎖裡等任何東西，
//! 所以直接同步做完、回一個已完成的 future，跟巨集展開出來的簽名一致。
//!
//! 鎖用 `std::sync::Mutex` 而不是 tokio 的：臨界區只有幾次 HashMap 操作、
//! 從不跨 `await`，用非同步鎖只會多一次排程。中毒（持鎖時 panic）就直接
//! 接手內容 —— 每個操作都是單一步驟，不會留下半套狀態。

use std::collections::HashMap;
use std::future::{Future, ready};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tower_sessions::cookie::time::{Duration, OffsetDateTime};
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

/// 踢人的警告最多多久印一次。被灌的時候每一筆新紀錄都會踢掉一筆，
/// 逐筆印只會把日誌淹掉；累計數量、一分鐘報一次就夠看出在發生什麼。
const EVICT_WARN_EVERY: Duration = Duration::minutes(1);

#[derive(Debug, Default)]
struct Inner {
    map: HashMap<Id, Record>,
    /// 上次印警告之後踢掉的筆數。
    evicted_since_warn: usize,
    last_warn: Option<OffsetDateTime>,
}

/// 見模組說明。`Clone` 共用同一份資料（跟 `MemoryStore` 一樣），
/// 才能一份交給 `SessionManagerLayer`、一份留給背景的 `sweep`。
#[derive(Clone, Debug)]
pub struct BoundedMemoryStore {
    inner: Arc<Mutex<Inner>>,
    max: usize,
}

/// `#[async_trait]` 展開後的回傳型別。
type BoxFut<'a, T> = Pin<Box<dyn Future<Output = session_store::Result<T>> + Send + 'a>>;

impl BoundedMemoryStore {
    /// `max` 是同時存在的紀錄上限，至少 1。
    pub fn new(max: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            max: max.max(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 目前存放的紀錄數（含尚未被掃到的過期紀錄）。
    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    /// 刪掉所有已過期的紀錄，回傳刪掉幾筆。
    pub fn sweep(&self) -> usize {
        let now = OffsetDateTime::now_utc();
        let mut g = self.lock();
        let before = g.map.len();
        g.map.retain(|_, r| is_active(r, now));
        before - g.map.len()
    }

    /// 放進一筆**新的** id（呼叫端保證 `map` 裡沒有它），必要時先騰出位子。
    fn insert_new(&self, g: &mut Inner, record: &Record) {
        if g.map.len() >= self.max {
            let now = OffsetDateTime::now_utc();
            g.map.retain(|_, r| is_active(r, now));
        }
        let mut evicted = 0;
        while g.map.len() >= self.max {
            // 到期最早的那筆：在限流之下這只會是別的匿名紀錄。
            let Some(victim) = g.map.values().min_by_key(|r| r.expiry_date).map(|r| r.id) else {
                break;
            };
            g.map.remove(&victim);
            evicted += 1;
        }
        if evicted > 0 {
            self.note_eviction(g, evicted);
        }
        g.map.insert(record.id, record.clone());
    }

    fn note_eviction(&self, g: &mut Inner, evicted: usize) {
        g.evicted_since_warn += evicted;
        let now = OffsetDateTime::now_utc();
        let due = g.last_warn.is_none_or(|t| now - t >= EVICT_WARN_EVERY);
        if due {
            tracing::warn!(
                "session store 已達上限 {}，這一分鐘踢掉 {} 筆最早到期的紀錄 —— 有人在灌匿名登入起手？",
                self.max,
                g.evicted_since_warn
            );
            g.evicted_since_warn = 0;
            g.last_warn = Some(now);
        }
    }
}

fn is_active(record: &Record, now: OffsetDateTime) -> bool {
    record.expiry_date > now
}

// 簽名照 `#[async_trait]` 展開的樣子寫：三個生命期都出現在 where 子句裡
// （early-bound），少一個或合成一個編譯器都會說跟 trait 對不上（E0195）。
impl SessionStore for BoundedMemoryStore {
    fn create<'s, 'r, 'f>(&'s self, record: &'r mut Record) -> BoxFut<'f, ()>
    where
        's: 'f,
        'r: 'f,
        Self: 'f,
    {
        let mut g = self.lock();
        // id 碰撞就換一個（跟 MemoryStore 一樣）：id 是 128 位元隨機數，
        // 這個迴圈實務上不會跑第二圈，但撞到就覆寫別人的 session 不可接受。
        while g.map.contains_key(&record.id) {
            record.id = Id::default();
        }
        self.insert_new(&mut g, record);
        Box::pin(ready(Ok(())))
    }

    fn save<'s, 'r, 'f>(&'s self, record: &'r Record) -> BoxFut<'f, ()>
    where
        's: 'f,
        'r: 'f,
        Self: 'f,
    {
        let mut g = self.lock();
        if let Some(slot) = g.map.get_mut(&record.id) {
            // 既有 session 更新內容，不會讓 store 變大，不必騰位子。
            *slot = record.clone();
        } else {
            self.insert_new(&mut g, record);
        }
        Box::pin(ready(Ok(())))
    }

    fn load<'s, 'r, 'f>(&'s self, session_id: &'r Id) -> BoxFut<'f, Option<Record>>
    where
        's: 'f,
        'r: 'f,
        Self: 'f,
    {
        let mut g = self.lock();
        let now = OffsetDateTime::now_utc();
        let found = match g.map.get(session_id) {
            Some(r) if is_active(r, now) => Some(r.clone()),
            Some(_) => {
                // 過期了就當場刪掉：不然要等到 sweep 才會釋放。
                g.map.remove(session_id);
                None
            }
            None => None,
        };
        Box::pin(ready(Ok(found)))
    }

    fn delete<'s, 'r, 'f>(&'s self, session_id: &'r Id) -> BoxFut<'f, ()>
    where
        's: 'f,
        'r: 'f,
        Self: 'f,
    {
        self.lock().map.remove(session_id);
        Box::pin(ready(Ok(())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(expires_in: Duration) -> Record {
        Record {
            id: Id::default(),
            data: Default::default(),
            expiry_date: OffsetDateTime::now_utc() + expires_in,
        }
    }

    /// 塞 max+1 筆活的紀錄：留下 max 筆，被踢的是到期最早的那一筆。
    #[tokio::test]
    async fn over_capacity_evicts_the_soonest_to_expire() {
        let store = BoundedMemoryStore::new(3);
        let soonest = record(Duration::minutes(5));
        let mut others = vec![record(Duration::minutes(30)), record(Duration::minutes(60))];
        for r in std::iter::once(&soonest).chain(others.iter()) {
            store.save(r).await.unwrap();
        }
        assert_eq!(store.len(), 3);

        let mut newcomer = record(Duration::minutes(45));
        store.create(&mut newcomer).await.unwrap();
        assert_eq!(store.len(), 3, "不能超過上限");
        assert!(
            store.load(&soonest.id).await.unwrap().is_none(),
            "到期最早的要被踢"
        );
        others.push(newcomer);
        for r in &others {
            assert!(store.load(&r.id).await.unwrap().is_some(), "其他的要還在");
        }
    }

    /// 滿了先清過期的，還有空位就不必踢活的。
    #[tokio::test]
    async fn expired_records_are_cleared_before_live_ones_are_evicted() {
        let store = BoundedMemoryStore::new(3);
        let live = record(Duration::minutes(1));
        store.save(&live).await.unwrap();
        store.save(&record(-Duration::seconds(1))).await.unwrap();
        store.save(&record(-Duration::minutes(5))).await.unwrap();
        assert_eq!(store.len(), 3);

        let mut newcomer = record(Duration::minutes(30));
        store.create(&mut newcomer).await.unwrap();
        assert_eq!(store.len(), 2, "兩筆過期的清掉、新的放進去");
        assert!(
            store.load(&live.id).await.unwrap().is_some(),
            "活的那筆雖然最早到期也不該被踢"
        );
    }

    /// 更新既有 session 不算新增，滿了也不會踢別人。
    #[tokio::test]
    async fn saving_an_existing_record_does_not_evict() {
        let store = BoundedMemoryStore::new(2);
        let a = record(Duration::minutes(5));
        let b = record(Duration::minutes(10));
        store.save(&a).await.unwrap();
        store.save(&b).await.unwrap();
        let a2 = Record {
            expiry_date: a.expiry_date + Duration::minutes(1),
            ..a.clone()
        };
        store.save(&a2).await.unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(store.load(&a.id).await.unwrap(), Some(a2));
        assert!(store.load(&b.id).await.unwrap().is_some());
    }

    /// 過期紀錄 `load` 回 None，而且當場被移除。
    #[tokio::test]
    async fn loading_an_expired_record_removes_it() {
        let store = BoundedMemoryStore::new(10);
        let stale = record(-Duration::seconds(1));
        store.save(&stale).await.unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.load(&stale.id).await.unwrap(), None);
        assert_eq!(store.len(), 0, "過期的不該留著等 sweep");
    }

    #[tokio::test]
    async fn sweep_removes_exactly_the_expired_records() {
        let store = BoundedMemoryStore::new(10);
        let live = record(Duration::minutes(5));
        store.save(&live).await.unwrap();
        store.save(&record(-Duration::seconds(1))).await.unwrap();
        store.save(&record(-Duration::hours(1))).await.unwrap();
        assert_eq!(store.sweep(), 2);
        assert_eq!(store.len(), 1);
        assert_eq!(store.load(&live.id).await.unwrap(), Some(live));
        assert_eq!(store.sweep(), 0, "沒東西可掃就回 0");
    }

    /// id 撞到既有紀錄時要換一個，不能覆寫別人的 session。
    #[tokio::test]
    async fn create_replaces_a_colliding_id() {
        let store = BoundedMemoryStore::new(10);
        let mut first = record(Duration::minutes(5));
        store.create(&mut first).await.unwrap();
        let mut second = record(Duration::minutes(5));
        second.id = first.id;
        store.create(&mut second).await.unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.load(&first.id).await.unwrap(),
            Some(first),
            "原本那筆要原封不動"
        );
    }

    #[tokio::test]
    async fn delete_removes_the_record() {
        let store = BoundedMemoryStore::new(10);
        let mut r = record(Duration::minutes(5));
        store.create(&mut r).await.unwrap();
        store.delete(&r.id).await.unwrap();
        assert_eq!(store.load(&r.id).await.unwrap(), None);
        assert_eq!(store.len(), 0);
    }
}
