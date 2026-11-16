//! A terminal's long history, kept compactly. The terminal emulator holds
//! only its last few hundred rows (32 bytes a cell, full width: a busy
//! 200-column terminal's 3,000 rows were ~19 MB); every row it lets go of
//! comes here as the bytes that draw it (text and colours, ~100–200 bytes),
//! in blocks deflated once full, read back a block at a time when you scroll
//! that far up (`TerminalPane`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read as _, Write as _};

/// Lines per deflated block.
const BLOCK: usize = 256;

pub struct History {
    /// Full blocks, oldest first.
    blocks: VecDeque<Block>,
    /// The newest lines, not a block yet.
    open: Vec<Vec<u8>>,
    /// Most lines kept; past it the oldest block goes.
    cap: usize,
    /// Every line ever pushed (how much arrived, even once it's full).
    pushed: u64,
    /// The last block read back, by its number.
    cache: RefCell<Option<(u64, Vec<Vec<u8>>)>>,
}

struct Block {
    /// Its number (blocks are numbered as made), for the cache.
    id: u64,
    deflated: Vec<u8>,
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self { blocks: VecDeque::new(), open: Vec::new(), cap, pushed: 0, cache: RefCell::new(None) }
    }

    /// Lines held (at most `cap`; it drops a whole block at a time).
    pub fn len(&self) -> usize {
        self.blocks.len() * BLOCK + self.open.len()
    }

    /// Every line ever pushed.
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    pub fn push(&mut self, line: Vec<u8>) {
        self.pushed += 1;
        self.open.push(line);
        if self.open.len() == BLOCK {
            let id = self.pushed / BLOCK as u64;
            let deflated = deflate(&std::mem::take(&mut self.open));
            self.blocks.push_back(Block { id, deflated });
        }
        while self.len() > self.cap && !self.blocks.is_empty() {
            self.blocks.pop_front();
        }
    }

    /// Line `i`, oldest first (0 = the oldest kept).
    pub fn get(&self, i: usize) -> Option<Vec<u8>> {
        let in_blocks = self.blocks.len() * BLOCK;
        if i >= in_blocks {
            return self.open.get(i - in_blocks).cloned();
        }
        let block = self.blocks.get(i / BLOCK)?;
        let mut cache = self.cache.borrow_mut();
        if cache.as_ref().is_none_or(|(id, _)| *id != block.id) {
            *cache = Some((block.id, inflate(&block.deflated)));
        }
        cache.as_ref().and_then(|(_, lines)| lines.get(i % BLOCK).cloned())
    }

    /// What it takes in memory (for the log).
    pub fn bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.deflated.len()).sum::<usize>() + self.open.iter().map(Vec::len).sum::<usize>()
    }
}

/// Lines as length-prefixed bytes, deflated (fast: it runs as output arrives).
fn deflate(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    for line in lines {
        let _ = z.write_all(&(line.len() as u32).to_le_bytes());
        let _ = z.write_all(line);
    }
    z.finish().unwrap_or_default()
}

fn inflate(deflated: &[u8]) -> Vec<Vec<u8>> {
    let mut raw = Vec::new();
    if flate2::read::ZlibDecoder::new(deflated).read_to_end(&mut raw).is_err() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut rest = raw.as_slice();
    while let Some((len, after)) = rest.split_first_chunk::<4>() {
        let len = u32::from_le_bytes(*len) as usize;
        let Some((line, after)) = after.split_at_checked(len) else { break };
        lines.push(line.to_vec());
        rest = after;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(i: usize) -> Vec<u8> {
        format!("\x1b[3{}mline {i} {}\x1b[m", i % 7, "x".repeat(i % 150)).into_bytes()
    }

    #[test]
    fn every_line_reads_back_as_pushed() {
        let mut h = History::new(10_000);
        for i in 0..1_000 {
            h.push(line(i));
        }
        assert_eq!(h.len(), 1_000);
        assert_eq!(h.pushed(), 1_000);
        for i in [0, 1, 255, 256, 511, 767, 768, 999] {
            assert_eq!(h.get(i), Some(line(i)), "line {i}");
        }
        assert_eq!(h.get(1_000), None);
        // Back and forth across blocks: the cache follows.
        assert_eq!(h.get(3), Some(line(3)));
        assert_eq!(h.get(700), Some(line(700)));
        assert_eq!(h.get(4), Some(line(4)));
    }

    #[test]
    fn past_its_cap_the_oldest_go_and_the_count_goes_on() {
        let mut h = History::new(600);
        for i in 0..2_000 {
            h.push(line(i));
        }
        assert!(h.len() <= 600 && h.len() > 600 - BLOCK, "{}", h.len());
        assert_eq!(h.pushed(), 2_000);
        let newest = h.len() - 1;
        assert_eq!(h.get(newest), Some(line(1_999)), "the newest is kept");
        assert_eq!(h.get(0), Some(line(2_000 - h.len())), "the oldest kept is in order");
    }

    #[test]
    fn it_is_much_smaller_than_the_cells_were() {
        let mut h = History::new(10_000);
        for i in 0..3_000 {
            h.push(line(i));
        }
        // The emulator kept these as 200 cells of 32 bytes each: ~19 MB.
        assert!(h.bytes() < 1_000_000, "{} bytes", h.bytes());
    }
}
