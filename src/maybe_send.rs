//! `Send` and `Sync` where the call runs on several threads (natively, on the app's Tokio runtime), and nothing in the
//! wasm32 build, whose JavaScript objects are bound to one thread.

#[cfg(native)]
mod bound {
    /// `Send` natively.
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}

    /// `Sync` natively.
    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}
}

#[cfg(web)]
mod bound {
    /// Nothing in the wasm32 build.
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}

    /// Nothing in the wasm32 build.
    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}
}

pub use bound::{MaybeSend, MaybeSync};
