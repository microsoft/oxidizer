// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use bytesbuf::{BytesBuf, BytesView};

use crate::error::Result;
use crate::output::Output;

mod sealed {
    pub trait Compression {}

    #[cfg(feature = "brotli")]
    impl Compression for crate::brotli::Compressor {}
    #[cfg(feature = "brotli")]
    impl Compression for crate::brotli::Decompressor {}
    #[cfg(feature = "deflate")]
    impl Compression for crate::deflate::Compressor {}
    #[cfg(feature = "deflate")]
    impl Compression for crate::deflate::Decompressor {}
    #[cfg(feature = "gzip")]
    impl Compression for crate::gzip::Compressor {}
    #[cfg(feature = "gzip")]
    impl Compression for crate::gzip::Decompressor {}
    #[cfg(feature = "zlib")]
    impl Compression for crate::zlib::Compressor {}
    #[cfg(feature = "zlib")]
    impl Compression for crate::zlib::Decompressor {}
    #[cfg(feature = "zstd")]
    impl Compression for crate::zstd::Compressor {}
    #[cfg(feature = "zstd")]
    impl Compression for crate::zstd::Decompressor {}

    impl<D> Compression for Box<dyn super::Compression<Mode = D>> {}
    impl Compression for Box<dyn super::Compressing> {}
    impl Compression for Box<dyn super::Decompressing> {}
}

/// Marks a [`Compression`] implementation that compresses its input.
///
/// This marker cannot be constructed outside this crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct Compress;

/// Marks a [`Compression`] implementation that decompresses its input.
///
/// This marker cannot be constructed outside this crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct Decompress;

/// A streaming compression or decompression operation.
///
/// Every format's compressor and decompressor implements this contract. The `Mode` associated type
/// records which operation an implementation performs without changing how callers drive it. This
/// allows shared processing code to accept any `Compression`, while APIs that require one direction
/// can use `Compression<Mode = Compress>` or `Compression<Mode = Decompress>`.
///
/// The trait is sealed so formats and methods can be added without breaking downstream code.
/// Every implementation is `Send + Sync`.
///
/// # Examples
///
/// ```
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::{GlobalPool, MemoryShared};
/// use compressors::{Compress, Compression, Output, gzip};
///
/// fn compress(
///     mut compression: impl Compression<Mode = Compress>,
///     input: BytesView,
/// ) -> compressors::Result<BytesView> {
///     compression.process(input)
/// }
///
/// let memory = GlobalPool::new();
/// let compressed = compress(
///     gzip::Compressor::new(memory.clone()),
///     BytesView::copied_from_slice(b"format agnostic", &memory),
/// )?;
///
/// assert_eq!(compressed.range(0..2).to_vec(), vec![0x1f, 0x8b]);
/// # Ok::<(), compressors::Error>(())
/// ```
pub trait Compression: sealed::Compression + fmt::Debug + Send + Sync {
    /// Whether this implementation compresses or decompresses its input.
    type Mode;

    /// Supplies more input.
    ///
    /// # Errors
    ///
    /// Returns an error if input is still pending or end of input has been signalled.
    fn push(&mut self, input: BytesView) -> Result<()>;

    /// Signals that no further input will be supplied.
    fn end_input(&mut self);

    /// Produces the next output chunk.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying engine fails or the input is invalid.
    fn pull(&mut self) -> Result<Output>;

    /// Processes one complete input and returns the whole result.
    ///
    /// This is shorthand for [`push`][Compression::push], [`end_input`][Compression::end_input], and
    /// draining [`pull`][Compression::pull]. It ends the operation, so an implementation serves
    /// one call. Drive `pull` directly to keep memory bounded by the configured chunk size.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying engine fails or the input is invalid.
    fn process(mut self, input: BytesView) -> Result<BytesView>
    where
        Self: Sized,
    {
        self.push(input)?;
        self.end_input();

        let mut collected = BytesBuf::new();
        loop {
            match self.pull()? {
                Output::Data(chunk) => collected.put_bytes(chunk),
                Output::Progress => {}
                Output::Done => break,
                Output::NeedInput => {
                    return Err(crate::Error::invalid_state("the operation requested input after end of input"));
                }
            }
        }

        Ok(collected.consume_all())
    }

    /// Compresses one complete input and returns the whole result.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying compression engine fails.
    fn compress(self, input: BytesView) -> Result<BytesView>
    where
        Self: Sized + Compression<Mode = Compress>,
    {
        self.process(input)
    }

    /// Decompresses one complete input and returns the whole result.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is invalid, truncated, or exceeds the configured limits.
    fn decompress(self, input: BytesView) -> Result<BytesView>
    where
        Self: Sized + Compression<Mode = Decompress>,
    {
        self.process(input)
    }
}

/// Additional operations available while compressing.
pub trait Compressing: Compression<Mode = Compress> {
    /// Requests a resumable flush of all input supplied so far.
    ///
    /// Drain [`Compression::pull`] until it reports [`Output::NeedInput`] before pushing more
    /// input. Flushing can reduce the compression ratio.
    ///
    /// # Errors
    ///
    /// Returns an invalid-state error after end of input or a previous operation failure.
    fn flush(&mut self) -> Result<()>;
}

/// Additional operations available while decompressing.
pub trait Decompressing: Compression<Mode = Decompress> {
    /// Takes bytes already buffered after a completed single compressed stream.
    ///
    /// # Errors
    ///
    /// Returns an invalid-state error until decompression reports [`Output::Done`].
    fn take_remainder(&mut self) -> Result<BytesView>;
}

/// Implements the shared trait for a format module's compressor and decompressor.
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
macro_rules! impl_compression {
    ($($module:ident),+ $(,)?) => {
        $(
            impl Compression for crate::$module::Compressor {
                type Mode = Compress;

                fn push(&mut self, input: BytesView) -> Result<()> {
                    Self::push(self, input)
                }

                fn end_input(&mut self) {
                    Self::end_input(self);
                }

                fn pull(&mut self) -> Result<Output> {
                    Self::pull(self)
                }
            }

            impl Compressing for crate::$module::Compressor {
                fn flush(&mut self) -> Result<()> {
                    Self::flush(self)
                }
            }

            impl Compression for crate::$module::Decompressor {
                type Mode = Decompress;

                fn push(&mut self, input: BytesView) -> Result<()> {
                    Self::push(self, input)
                }

                fn end_input(&mut self) {
                    Self::end_input(self);
                }

                fn pull(&mut self) -> Result<Output> {
                    Self::pull(self)
                }

            }

            impl Decompressing for crate::$module::Decompressor {
                fn take_remainder(&mut self) -> Result<BytesView> {
                    Self::take_remainder(self)
                }
            }
        )+
    };
}

#[cfg(feature = "brotli")]
impl_compression!(brotli);
#[cfg(feature = "deflate")]
impl_compression!(deflate);
#[cfg(feature = "gzip")]
impl_compression!(gzip);
#[cfg(feature = "zlib")]
impl_compression!(zlib);
#[cfg(feature = "zstd")]
impl_compression!(zstd);

impl<D> Compression for Box<dyn Compression<Mode = D>> {
    type Mode = D;

    fn push(&mut self, input: BytesView) -> Result<()> {
        (**self).push(input)
    }

    fn end_input(&mut self) {
        (**self).end_input();
    }

    fn pull(&mut self) -> Result<Output> {
        (**self).pull()
    }
}

impl Compression for Box<dyn Compressing> {
    type Mode = Compress;

    fn push(&mut self, input: BytesView) -> Result<()> {
        (**self).push(input)
    }

    fn end_input(&mut self) {
        (**self).end_input();
    }

    fn pull(&mut self) -> Result<Output> {
        (**self).pull()
    }
}

impl Compressing for Box<dyn Compressing> {
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl Compression for Box<dyn Decompressing> {
    type Mode = Decompress;

    fn push(&mut self, input: BytesView) -> Result<()> {
        (**self).push(input)
    }

    fn end_input(&mut self) {
        (**self).end_input();
    }

    fn pull(&mut self) -> Result<Output> {
        (**self).pull()
    }
}

impl Decompressing for Box<dyn Decompressing> {
    fn take_remainder(&mut self) -> Result<BytesView> {
        (**self).take_remainder()
    }
}

#[cfg(all(test, feature = "gzip"))]
#[derive(Debug)]
pub(crate) struct ProgressCompression {
    pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(all(test, feature = "gzip"))]
impl ProgressCompression {
    pub(crate) fn new(pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self { pulls }
    }
}

#[cfg(all(test, feature = "gzip"))]
impl sealed::Compression for ProgressCompression {}

#[cfg(all(test, feature = "gzip"))]
impl Compression for ProgressCompression {
    type Mode = Compress;

    fn push(&mut self, _input: BytesView) -> Result<()> {
        Ok(())
    }

    fn end_input(&mut self) {}

    fn pull(&mut self) -> Result<Output> {
        self.pulls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Output::Progress)
    }
}

#[cfg(all(test, feature = "gzip"))]
mod tests {
    use bytesbuf::mem::GlobalPool;

    use super::*;
    use crate::format::Format;
    use crate::gzip;

    fn view(bytes: &[u8]) -> BytesView {
        BytesView::copied_from_slice(bytes, &GlobalPool::new())
    }

    #[test]
    fn round_trips_through_the_trait_alone() {
        let memory = GlobalPool::new();

        let mut compressor: Box<dyn Compression<Mode = Compress>> = Box::new(gzip::Compressor::new(memory.clone()));
        Compression::push(&mut *compressor, view(b"driven through the trait")).expect("push succeeds");
        Compression::end_input(&mut *compressor);

        let mut collected = BytesBuf::new();
        loop {
            match Compression::pull(&mut *compressor).expect("pull succeeds") {
                Output::Data(chunk) => collected.put_bytes(chunk),
                Output::Progress => {}
                Output::NeedInput => panic!("compressor requested input after end"),
                Output::Done => break,
            }
        }

        let mut decompressor: Box<dyn Compression<Mode = Decompress>> = Box::new(gzip::Decompressor::new(memory));
        Compression::push(&mut *decompressor, collected.consume_all()).expect("push succeeds");
        Compression::end_input(&mut *decompressor);

        let mut plain = BytesBuf::new();
        loop {
            match Compression::pull(&mut *decompressor).expect("pull succeeds") {
                Output::Data(chunk) => plain.put_bytes(chunk),
                Output::Progress => {}
                Output::NeedInput => panic!("decompressor requested input after end"),
                Output::Done => break,
            }
        }

        assert_eq!(plain.consume_all().to_vec(), b"driven through the trait".to_vec());
    }

    #[test]
    fn trait_objects_are_send_sync_and_debug() {
        fn assert_send_sync<T: Send + Sync + ?Sized>(_: &T) {}

        let memory = GlobalPool::new();
        let compressor: Box<dyn Compression<Mode = Compress>> = Box::new(gzip::Compressor::new(memory.clone()));
        let decompressor: Box<dyn Compression<Mode = Decompress>> = Box::new(gzip::Decompressor::new(memory));

        assert_send_sync(&*compressor);
        assert_send_sync(&*decompressor);
        assert_send_sync(&gzip::Compressor::new(GlobalPool::new()));
        assert_send_sync(&gzip::Decompressor::new(GlobalPool::new()));
        assert!(format!("{compressor:?}").contains("Compressor"));
        assert!(format!("{decompressor:?}").contains("Decompressor"));
    }

    #[test]
    fn direction_specific_traits_work_for_concrete_and_runtime_operations() {
        let memory = GlobalPool::new();
        let input = view(b"direction-specific capabilities");

        let mut concrete = gzip::Compressor::new(memory.clone());
        concrete.push(input.clone()).expect("push succeeds");
        Compressing::flush(&mut concrete).expect("concrete flush succeeds");
        loop {
            match concrete.pull().expect("pull succeeds") {
                Output::Data(_) | Output::Progress => {}
                Output::NeedInput => break,
                Output::Done => panic!("flush ended the stream"),
            }
        }

        let mut compressor = Format::Gzip.compressor().build(memory.clone());
        compressor.push(input).expect("push succeeds");
        Compressing::flush(&mut compressor).expect("boxed flush succeeds");
        let mut compressed = BytesBuf::new();
        loop {
            match compressor.pull().expect("pull succeeds") {
                Output::Data(chunk) => compressed.put_bytes(chunk),
                Output::Progress => {}
                Output::NeedInput => break,
                Output::Done => panic!("flush ended the stream"),
            }
        }

        compressor.end_input();
        loop {
            match compressor.pull().expect("pull succeeds") {
                Output::Data(chunk) => compressed.put_bytes(chunk),
                Output::Progress => {}
                Output::NeedInput => panic!("compressor requested input after end"),
                Output::Done => break,
            }
        }

        let trailing = view(b"trailing");
        let joined = BytesView::from_views([compressed.consume_all(), trailing.clone()]);
        let mut decompressor = Format::Gzip.decompressor().multi_stream(false).build(memory);
        decompressor.push(joined).expect("push succeeds");
        loop {
            match decompressor.pull().expect("pull succeeds") {
                Output::Data(_) | Output::Progress => {}
                Output::NeedInput => panic!("complete stream requested more input"),
                Output::Done => break,
            }
        }

        assert_eq!(
            Decompressing::take_remainder(&mut decompressor)
                .expect("boxed remainder succeeds")
                .to_vec(),
            trailing.to_vec()
        );
    }
}
