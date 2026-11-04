//! Platform-independent heart of Tengen.
//!
//! - [`board`]: stones, liberties, captures, ko and suicide.
//! - [`game`]: a game record (metadata + moves) with replay to any move.
//! - [`sgf`]: reading and writing SGF files.
//! - [`adapter`]: the interface every Go server adapter implements.

pub mod adapter;
pub mod board;
pub mod game;
pub mod sgf;

pub use board::{Board, Color, MoveError, Point};
pub use game::{Game, GameInfo, Move};
