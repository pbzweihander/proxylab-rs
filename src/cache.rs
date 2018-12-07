use crate::*;
use futures::lock::Mutex;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

const MAX_CACHE_SIZE: usize = 1000000;
const MAX_CONTENT_SIZE: usize = 200000;

struct Cache {
    pub inner: BTreeMap<Uri, Response>,
    pub keys: VecDeque<Uri>,
    pub size: usize,
}

impl Cache {
    fn new() -> Self {
        Cache {
            inner: BTreeMap::new(),
            keys: VecDeque::new(),
            size: 0,
        }
    }
}

lazy_static! {
    static ref CACHE: Arc<Mutex<Cache>> = Arc::new(Mutex::new(Cache::new()));
}

pub async fn find_cache_block(uri: Uri) -> Option<Response> {
    let cache: &Cache = &*await!(CACHE.lock());

    let resp = cache.inner.get(&uri).map(|r| r.clone());
    if let Some(_) = resp.as_ref() {
        println!("cache hit: {}\n", uri.to_string());
    }
    resp
}

async fn cache_replacement_policy() {
    let cache: &mut Cache = &mut *await!(CACHE.lock());

    while cache.size > MAX_CACHE_SIZE && !cache.keys.is_empty() {
        let head = cache.keys.pop_front().unwrap();
        println!("cache removed: {}\n", head.to_string());
        let resp = cache.inner.remove(&head);
        if let Some(resp) = resp {
            cache.size -= resp.content.len();
        }
    }
}

pub async fn add_cache_block(uri: Uri, resp: Response) {
    if resp.content.len() > MAX_CONTENT_SIZE {
        return;
    }

    let mut cache_lock = await!(CACHE.lock());
    let cache: &mut Cache = &mut *cache_lock;

    println!("cache saved: {}\n", uri.to_string());
    cache.keys.push_back(uri.clone());
    cache.size += resp.content.len();
    let old_resp = cache.inner.insert(uri, resp);
    if let Some(old_resp) = old_resp {
        cache.size -= old_resp.content.len();
    }

    println!("{}", cache.size);

    if cache.size > MAX_CACHE_SIZE {
        drop(cache_lock);
        await!(cache_replacement_policy());
    }
}
