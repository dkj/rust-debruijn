#![allow(dead_code)]

//! # debruijn-rs: a De Bruijn graph for DNA seqeunces in Rust.
//! This library provides tools for efficiently construction de bruijn graphs
//! from DNA sequences, tracking arbitrary metadata associated with kmers in the
//! graph, and performing path-compression of unbranched graph paths to improve
//! speed and reduce memory consumption.

//! All the data structures in debruijn-rs are specialized to the alphabet {'A', 'C', 'G', 'T'},
//! and use 2-bit packed encoding of base-pairs into integer types, and efficient methods for
//! reverse complement, enumerating kmers from longer sequences, and transfering data between
//! sequences. 

extern crate num;
extern crate extprim;
extern crate rand;
extern crate linked_hash_map;
#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate smallvec;
extern crate bit_set;
extern crate itertools;

use std::hash::Hash;
use std::fmt;

pub mod kmer;
pub mod dna_string;
pub mod paths;
pub mod vmer;
pub mod msp;
pub mod filter;
mod fx;
mod test;

/// Convert a 2-bit representation of a base to a char
pub fn bits_to_ascii(c: u8) -> u8 {
    match c {
        0u8 => 'A' as u8,
        1u8 => 'C' as u8,
        2u8 => 'G' as u8,
        3u8 => 'T' as u8,
        _ => 'X' as u8,
    }
}

/// Convert an ASCII-encoded DNA base to a 2-bit representation
pub fn base_to_bits(c: u8) -> u8 {
    match c {
        b'A' => 0u8,
        b'C' => 1u8,
        b'G' => 2u8,
        b'T' => 3u8,
        _ => 0u8,
    }
}


/// Convert a 2-bit representation of a base to a char
pub fn bits_to_base(c: u8) -> char {
    match c {
        0u8 => 'A',
        1u8 => 'C',
        2u8 => 'G',
        3u8 => 'T',
        _ => 'X',
    }
}

/// The complement of a 2-bit encoded base
pub fn complement(base: u8) -> u8 {
    (!base) & 0x3u8
}


/// Generic trait for interacting with DNA sequences
pub trait Mer: Sized + fmt::Debug {
    /// Length of DNA sequence
    fn len(&self) -> usize;

    /// Get 2-bit encoded base at position `pos`
    fn get(&self, pos: usize) -> u8;

    /// Set base at `pos` to 2-bit encoded base `val`
    fn set_mut(&mut self, pos: usize, val: u8);

    /// Set `nbases` positions in the sequence, starting at `pos`.
    /// Values must  be packed into the upper-most bits of `value`.
    fn set_slice_mut(&mut self, pos: usize, nbases: usize, value: u64);

    /// Return a new object containing the reverse complement of the sequence
    fn rc(&self) -> Self;

    /// Add the base `v` to the left side of the sequence, and remove the rightmost base
    fn extend_left(&self, v: u8) -> Self;

    /// Add the base `v` to the right side of the sequence, and remove the leftmost base
    fn extend_right(&self, v: u8) -> Self;

    /// Add the base `v` to the side of sequence given by `dir`, and remove a base at the opposite side
    fn extend(&self, v: u8, dir: Dir) -> Self {
        match dir {
            Dir::Left => self.extend_left(v),
            Dir::Right => self.extend_right(v),
        }
    }

    /// Generate all the extension of this sequence given by `exts` in direction `Dir`
    fn get_extensions(&self, exts: Exts, dir: Dir) -> Vec<Self> {
        let ext_bases = exts.get(dir);
        ext_bases
            .iter()
            .map(|b| self.extend(b.clone(), dir))
            .collect()
    }
}

/// Encapsulates a Kmer sequence with statically known K.
pub trait Kmer: Mer + Sized + Copy + PartialEq + PartialOrd + Eq + Ord + Hash {
    /// Create a Kmer initialized to all A's
    fn empty() -> Self;

    /// K value for this concrete type.
    fn k() -> usize;

    /// Return the minimum of the kmer and it's reverse complement, and a flag indicating if sequence was flipped
    fn min_rc_flip(&self) -> (Self, bool) {
        let rc = self.rc();
        if *self < rc {
            (self.clone(), false)
        } else {
            (rc, true)
        }
    }

    // Return the minimum of the kmer and it's reverse complement
    fn min_rc(&self) -> Self {
        let rc = self.rc();
        if *self < rc { self.clone() } else { rc }
    }

    /// Test if this Kmer and it's reverse complement are the same
    fn is_palindrome(&self) -> bool {
        self.len() % 2 == 0 && *self == self.rc()
    }

    /// Create a Kmer from the first K bytes of `bytes`
    fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < Self::k() {
            panic!("bytes not long enough to form kmer")
        }

        let mut k0 = Self::empty();

        for i in 0..Self::k() {
            k0.set_mut(i, bytes[i])
        }

        k0
    }

    fn from_ascii(bytes: &[u8]) -> Self {
        if bytes.len() < Self::k() {
            panic!("bytes not long enough to form kmer")
        }

        let mut k0 = Self::empty();

        for i in 0..Self::k() {
            k0.set_mut(i, base_to_bits(bytes[i]))
        }

        k0
    }

    fn to_string(&self) -> String {
        let mut s = String::new();
        for pos in 0..self.len() {
            s.push(bits_to_base(self.get(pos)))
        }
        s
    }

    /// Generate all kmers from string
    fn kmers_from_string(str: &[u8]) -> Vec<Self> {
        let mut r = Vec::new();

        if str.len() < Self::k() {
            return r;
        }

        let mut k0 = Self::empty();

        for i in 0..Self::k() {
            k0.set_mut(i, str[i]);
        }

        r.push(k0.clone());

        for i in Self::k()..str.len() {
            k0 = k0.extend_right(str[i]);
            r.push(k0.clone());
        }

        r
    }
}

/// An immutable interface to a Mer sequence.
pub trait MerImmut: Mer + Clone {
    fn set(&self, pos: usize, val: u8) -> Self {
        let mut new = self.clone();
        new.set_mut(pos, val);
        new
    }

    fn set_slice(&self, pos: usize, nbases: usize, bits: u64) -> Self {
        let mut new = self.clone();
        new.set_slice_mut(pos, nbases, bits);
        new
    }
}

impl<T> MerImmut for T where T: Mer + Clone {}


/// A DNA sequence with run-time variable length, up to a statically known maximum length
pub trait Vmer<K: Kmer>: Mer + PartialEq + Eq + Clone {

    /// Create a new sequence with length `len`, initialized to all A's
    fn new(len: usize) -> Self;

    /// Maximum sequence length that can be stored in this type
    fn max_len() -> usize;

    /// Efficiently extract a Kmer from the sequence
    fn get_kmer(&self, pos: usize) -> K;

    /// Get the first Kmer from the sequence
    fn first_kmer(&self) -> K {
        self.get_kmer(0)
    }

    /// Get the last kmer in the sequence
    fn last_kmer(&self) -> K {
        self.get_kmer(self.len() - K::k())
    }

    /// Get the terminal kmer of the sequence, on the side of the sequence given by dir
    fn term_kmer(&self, dir: Dir) -> K {
        match dir {
            Dir::Left => self.first_kmer(),
            Dir::Right => self.last_kmer(),
        }
    }

    /// Iterate over the kmers in the sequence
    fn iter_kmers(&self) -> KmerIter<K, Self> {
        KmerIter {
            bases: self,
            kmer: self.first_kmer(),
            pos: K::k(),
        }
    }

    /// Iterate over the kmers and their extensions, given the extension of the whole sequence
    fn iter_kmer_exts(&self, seq_exts: Exts) -> KmerExtsIter<K, Self> {
        KmerExtsIter {
            bases: self,
            exts: seq_exts,
            kmer: self.first_kmer(),
            pos: K::k(),
        }
    }

    /// Create a Vmer from a sequence of bytes
    fn from_slice(seq: &[u8]) -> Self {
        let mut vmer = Self::new(seq.len());
        for i in 0 .. seq.len() {
            vmer.set_mut(i, seq[i]);
        }

        vmer
    }
}


/// Direction of motion in a DeBruijn graph
#[derive(Copy, Clone, Debug)]
pub enum Dir {
    Left,
    Right,
}

impl Dir {
    /// Return a fresh Dir with the opposite direction
    pub fn flip(&self) -> Dir {
        match *self {
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
        }
    }

    /// Return a fresh Dir opposite direction if do_flip == True
    pub fn cond_flip(&self, do_flip: bool) -> Dir {
        if do_flip { self.flip() } else { *self }
    }

    /// Pick between two alternatives, depending on the direction
    pub fn pick<T>(&self, if_left: T, if_right: T) -> T {
        match self {
            &Dir::Left => if_left,
            &Dir::Right => if_right,
        }
    }
}


/// Store single-base extensions for a DNA Debruijn graph.
///
/// 8 bits, 4 higher order ones represent extensions to the right, 4 lower order ones
/// represent extensions to the left. For each direction the bits (from lower order
/// to higher order) represent whether there exists an extension with each of the
/// letters A, C, G, T. So overall the bits are:
///  right   left
/// T G C A T G C A
#[derive(Eq, PartialEq, Copy, Clone, Ord, PartialOrd, Hash)]
pub struct Exts {
    pub val: u8,
}

impl Exts {
    pub fn new(val: u8) -> Self {
        Exts { val: val }
    }

    pub fn empty() -> Exts {
        Exts { val: 0u8 }
    }

    pub fn from_single_dirs(left: Exts, right: Exts) -> Exts {
        Exts { val: (right.val << 4) | (left.val & 0xf) }
    }

    pub fn merge(left: Exts, right: Exts) -> Exts {
        Exts { val: left.val & 0x0f | right.val & 0xf0 }
    }

    pub fn add(&self, v: Exts) -> Exts {
        Exts { val: self.val | v.val }
    }

    pub fn set(&self, dir: Dir, pos: u8) -> Exts {
        let shift = pos +
                    match dir {
                        Dir::Right => 4,
                        Dir::Left => 0,
                    };

        let new_val = self.val | (1u8 << shift);
        Exts { val: new_val }
    }

    #[inline]
    fn dir_bits(&self, dir: Dir) -> u8 {
        match dir {
            Dir::Right => self.val >> 4,
            Dir::Left => self.val & 0xf,
        }
    }

    pub fn get(&self, dir: Dir) -> Vec<u8> {
        let bits = self.dir_bits(dir);
        let mut v = Vec::new();
        for i in 0..4 {
            if bits & (1 << i) > 0 {
                v.push(i);
            }
        }

        v
    }

    pub fn has_ext(&self, dir: Dir, base: u8) -> bool {
        let bits = self.dir_bits(dir);
        (bits & (1 << base)) > 0
    }

    pub fn from_slice_bounds(src: &[u8], start: usize, length: usize) -> Exts {
        let l_extend = if start > 0 {
            1u8 << (src[start - 1])
        } else {
            0u8
        };
        let r_extend = if start + length < src.len() {
            1u8 << src[start + length]
        } else {
            0u8
        };

        Exts { val: (r_extend << 4) | l_extend }
    }

    pub fn num_exts_l(&self) -> u8 {
        self.num_ext_dir(Dir::Left)
    }

    pub fn num_exts_r(&self) -> u8 {
        self.num_ext_dir(Dir::Right)
    }

    pub fn num_ext_dir(&self, dir: Dir) -> u8 {
        let e = self.dir_bits(dir);
        ((e & 1u8) >> 0) + ((e & 2u8) >> 1) + ((e & 4u8) >> 2) + ((e & 8u8) >> 3)
    }

    pub fn mk_left(base: u8) -> Exts {
        Exts::empty().set(Dir::Left, base)
    }
 
    pub fn mk_right(base: u8) -> Exts {
        Exts::empty().set(Dir::Right, base)
    }

    pub fn mk(left_base: u8, right_base: u8) -> Exts {
        Exts::merge(Exts::mk_left(left_base), Exts::mk_right(right_base))
    }

    pub fn get_unique_extension(&self, dir: Dir) -> Option<u8> {
        if self.num_ext_dir(dir) != 1 {
            None
        } else {
            let e = self.dir_bits(dir);
            for i in 0..4 {
                if (e & (1 << i)) > 0 {
                    return Some(i);
                }
            }

            None
        }
    }

    pub fn single_dir(&self, dir: Dir) -> Exts {
        match dir {
            Dir::Right => Exts { val: self.val >> 4 },
            Dir::Left => Exts { val: self.val & 0xfu8 },
        }
    }

    /// Complement the extension bases for each direction
    pub fn complement(&self) -> Exts {
        let v = self.val;

        // swap bits
        let mut r = (v & 0x55u8) << 1 | ((v >> 1) & 0x55u8);

        // swap pairs
        r = (r & 0x33u8) << 2 | ((r >> 2) & 0x33u8);
        Exts { val: r }
    }

    pub fn reverse(&self) -> Exts {
        let v = self.val;
        let r = (v & 0xf) << 4 | (v >> 4);
        Exts { val: r }
    }

    pub fn rc(&self) -> Exts {
        self.reverse().complement()
    }
}

impl fmt::Debug for Exts {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = String::new();

        for b in self.get(Dir::Left) {
            s.push(bits_to_base(b));
        }
        s.push('|');

        for b in self.get(Dir::Right) {
            s.push(bits_to_base(b));
        }

        write!(f, "{}", s)
    }
}



/// Iterate over the `Kmer`s of a DNA sequence efficiently
pub struct KmerIter<'a, K: Kmer, D>
    where D: 'a
{
    bases: &'a D,
    kmer: K,
    pos: usize,
}

impl<'a, K: Kmer, D: Mer> Iterator for KmerIter<'a, K, D> {
    type Item = K;

    fn next(&mut self) -> Option<K> {
        if self.pos <= self.bases.len() {
            let retval = self.kmer;

            if self.pos < self.bases.len(){
                self.kmer = self.kmer.extend_right(self.bases.get(self.pos));
            }
            
            self.pos = self.pos + 1;
            Some(retval)
        } else {
            None
        }
    }
}

/// Iterate over the `(Kmer, Exts)` tuples of a sequence and it's extensions efficiently
pub struct KmerExtsIter<'a, K: Kmer, D>
    where D: 'a
{
    bases: &'a D,
    exts: Exts,
    kmer: K,
    pos: usize,
}

impl<'a, K: Kmer, D: Mer> Iterator for KmerExtsIter<'a, K, D> {
    type Item = (K, Exts);

    fn next(&mut self) -> Option<(K,Exts)> {
        if self.pos <= self.bases.len() {

            let next_base = 
                if self.pos < self.bases.len() {
                    self.bases.get(self.pos)
                } else {
                    0u8
                };

            let cur_left = 
                if self.pos == K::k() {
                    self.exts
                } else {
                    Exts::mk_left(self.bases.get(self.pos - K::k() - 1))
                };

            let cur_right = 
                if self.pos < self.bases.len() {
                    Exts::mk_right(next_base)
                } else {
                    self.exts
                };
            
            let cur_exts = Exts::merge(cur_left, cur_right);

            let retval = self.kmer;
            self.kmer = self.kmer.extend_right(next_base);
            self.pos = self.pos + 1;
            Some((retval, cur_exts))
        } else {
            None
        }
    }
}


