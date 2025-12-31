use x86_64::align_down;

pub mod bitmap;
pub mod sync;


pub struct StackHelper<'a> {
    ptr: &'a mut u64,
}

impl<'a> StackHelper<'a> {
    pub fn new(ptr: &'a mut u64) -> StackHelper<'a> {
        StackHelper::<'a> { ptr }
    }

    pub fn skip_by(&mut self, by: u64) {
        *self.ptr -= by;
    }

    pub fn offset<T: Sized>(&mut self) -> &mut T {
        self.skip_by(size_of::<T>() as u64);
        unsafe {
            &mut *(*self.ptr as *mut T)
        }
    }

    pub fn top(&self) -> u64 {
        *self.ptr
    }

    pub unsafe fn write_slice<T: Sized>(&mut self, slice: &[T]) {
        self.write_bytes(slice_into_bytes(slice));
    }

    pub fn align_down(&mut self) {
        *self.ptr = align_down(*self.ptr, 16);
    }

    pub fn write<T: Sized>(&mut self, value: T) {
        self.skip_by(size_of::<T>() as u64);

        unsafe {
            (*self.ptr as *mut T).write(value);
        }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.skip_by(bytes.len() as u64);
        unsafe {
            (*self.ptr as *mut u8).copy_from(bytes.as_ptr(), bytes.len());
        }
    }

    pub fn get_by(&mut self, by: u64) {
        *self.ptr += by;
    }

    pub fn get<'b, T: Sized>(&mut self) -> &'b mut T {
        let x = unsafe { &mut *(*self.ptr as *mut T) };

        self.get_by(size_of::<T>() as u64);
        x
    }
}


pub fn slice_into_bytes<T: Sized>(slice: &[T]) -> &[u8] {
    let data = slice.as_ptr().cast::<u8>();
    let size = size_of_val(slice);

    unsafe { core::slice::from_raw_parts(data, size) }
}
