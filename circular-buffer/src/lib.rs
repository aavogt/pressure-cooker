#![feature(generic_const_exprs)]
#[derive(Debug)]
/// A circular buffer that holds at most N element.
/// The buffer is implemented as a fixed-size array of size 2*N.
/// Originally I used an array of size N, except I couldn't figure out the as_slice() method,
/// but embedded_graphics::Polyline::new (which takes a slice of points) might not be the only way
/// to draw a polyline.
/// Incidentally, this design trades space for time: rotate_left() is used once every N pushes on a full buffer.
pub struct CircularBuffer<T, const N : usize> where [T; 2*N] : {
    buffer: [T; 2*N],    // Fixed-size array
    start: usize,      // Index of the start of the buffer
    size: usize,        // number of elements in the buffer
}

impl<T : Default + Copy, const N : usize> CircularBuffer<T, N> where [T; 2*N] : {
    pub fn new() -> Self {
        CircularBuffer {
            buffer: [Default::default(); 2*N],
            start: 0,
            size: 0,
        }
    }

    pub fn is_full(&self) -> bool {
        self.size == N
    }

    // copy of the oldest element in the buffer
    pub fn last(&self) -> Option<T> {
        if self.size == 0 {
            None  // Buffer is empty
        } else {
            Some(self.buffer[self.start])
        }
    }

    // copy of the newest element to be added
    pub fn head(&self) -> Option<T> {
        if self.size == 0 {
            None  // Buffer is empty
        } else {
            Some(self.buffer[self.end() - 1])
        }
    }

    fn shift_start(&mut self) {
        self.buffer.rotate_left(self.start);
        self.start = 0;
    }

    /// index of the next element to be pushed
    fn end(&self) -> usize {
        self.start + self.size
    }

    pub fn push(&mut self, value: T) {
        self.buffer[self.end()] = value;
        if self.size == N {
            self.start+=1;
        };
        if self.size < N {
            self.size+=1;
        }
        if self.start + self.size >= 2*N {
            self.shift_start();
        }
    }


    pub fn into_iter<'a>(&'a self) -> CircularBufferIter<'a, T, N> where [T; 2*N] : {
        CircularBufferIter {
            buffer: &self,
            index: 0,
        }
    }

    /// xs.zip_with(ys,f) applies f to the oldest elements of xs and ys, then the next oldest, etc.
    /// until one of the buffers is exhausted.
    pub fn zip_with<U : Default+Copy, const M: usize>(&mut self, other: &CircularBuffer<U, M>, mut f: impl FnMut(&mut T, &U)) 
    where [(); 2*M]: {
        for i in 0..self.size.min(other.size) {
            let i1 = self.start + i;
            let i2 = other.start + i;
            f(&mut self.buffer[i1], &other.buffer[i2]);
        }
    }

    pub fn as_slice(&self) -> &[T] {
        &self.buffer[self.start..self.end()]
    }
}

pub struct CircularBufferIter<'a, T, const N : usize> where [T; 2*N] :{
    pub buffer: &'a CircularBuffer<T, N>,
    index: usize,
}

impl<'a, T : Default + Copy, const N : usize> Iterator for CircularBufferIter<'a, T, N> where [T; 2*N] :{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buffer.size == 0 || self.index >= self.buffer.size {
            None
        } else {
            let index = self.buffer.start + self.index;
            self.index += 1;
            Some(self.buffer.buffer[index])
        }
    }
}

impl<'a, T : Default + Copy, const N : usize> IntoIterator for &'a CircularBuffer<T, N> where [T; 2*N] :{
    type Item = T;
    type IntoIter = CircularBufferIter<'a, T, N>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_iter()
    }
}

// tests
#[cfg(test)]
mod tests {

    use super::*;
    use proptest::prelude::*;
    #[test]
    fn overfill1() {
        let mut buffer = CircularBuffer::<i32, 3>::new();
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
        buffer.push(4);
        assert_eq!(buffer.as_slice(), &[2, 3, 4]);
    }
    #[test]
    fn overfill2() {
        let mut buffer = CircularBuffer::<i32, 2>::new();
        for i in 0..10 {
            buffer.push(i);
        }
        assert_eq!(buffer.as_slice(), &[8, 9]);
    }

    #[test]

    fn zip_with() {
        let mut buffer1 = CircularBuffer::<i32, 3>::new();
        let mut buffer2 = CircularBuffer::<i32, 3>::new();
        buffer1.push(1);
        buffer1.push(2);
        buffer1.push(3);
        buffer2.push(4);
        buffer2.push(5);
        buffer2.push(6);
        buffer1.zip_with(&buffer2, |x, y| *x += *y);
        assert_eq!(buffer1.as_slice(), &[5, 7, 9]);
    }

    #[test]
    fn iter() {
        let mut buffer = CircularBuffer::<i32, 3>::new();
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
        let mut iter = buffer.into_iter();
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), None);
    }

    // proptest
    proptest! {
    #[test]
    fn push_get(s in 0..10u8) {
        let mut buffer = CircularBuffer::<u8, 3>::new();
        buffer.push(s);
        assert_eq!(buffer.head(), Some(s));
    }

    }
}
