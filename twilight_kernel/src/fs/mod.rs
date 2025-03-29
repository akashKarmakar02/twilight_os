use lazy_static::lazy_static;
use spin::Mutex;
use heapless::{LinearMap, String};

lazy_static! {
    pub static ref FS: Mutex<LinearMap<String<256>, File, 1024>> = Mutex::new(LinearMap::new());
}

#[derive(Clone)]
pub struct File {
    pathname: String<256>,
    content: String<1024>
}

impl File {
    pub fn create(path: &str) -> Option<Self> {
        let fs = FS.lock();

        if fs.contains_key(&String::from(path.parse().unwrap())) {
            None
        } else {
            Some(File {
                pathname: String::from(path.parse().unwrap()),
                content: String::new()
            })
        }
    }

    pub fn open(path: &str) -> Option<Self> {
        let fs = FS.lock();

        if fs.contains_key(&String::from(path.parse().unwrap())) {
            Some(fs.get(&String::from(path.parse().unwrap())).unwrap().clone())
        } else {
            None
        }
    }

    pub fn read(&mut self) -> String<1024> {
        self.content.clone()
    }

    pub fn write(&mut self, content: &str) {
        let mut fs = FS.lock();

        let _ = self.content.push_str(content);
        let _ = fs.insert(String::from(self.pathname.clone()), self.clone());
    }
}