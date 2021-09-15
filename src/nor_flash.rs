use crate::{iter::IterableByOverlaps, ReadStorage, Region, Storage};

/// Read only NOR flash trait.
pub trait ReadNorFlash {
	/// An enumeration of storage errors
	type Error;

	/// The minumum number of bytes the storage peripheral can read
	const READ_SIZE: usize;

	/// Read a slice of data from the storage peripheral, starting the read
	/// operation at the given address offset, and reading `bytes.len()` bytes.
	///
	/// This should throw an error in case `bytes.len()` will be larger than
	/// the peripheral end address.
	fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error>;

	/// The capacity of the peripheral in bytes.
	fn capacity(&self) -> usize;
}

/// NOR flash trait.
pub trait NorFlash: ReadNorFlash {
	/// The minumum number of bytes the storage peripheral can write
	const WRITE_SIZE: usize;

	/// The minumum number of bytes the storage peripheral can erase
	const ERASE_SIZE: usize;

	/// Erase the given storage range, clearing all data within `[from..to]`.
	/// The given range will contain all 1s afterwards.
	///
	/// This should return an error if the range is not aligned to a proper
	/// erase resolution
	/// If power is lost during erase, contents of the page are undefined.
	/// `from` and `to` must both be multiples of `ERASE_SIZE` and `from` <= `to`.
	fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error>;

	/// If power is lost during write, the contents of the written words are undefined,
	/// but the rest of the page is guaranteed to be unchanged.
	/// It is not allowed to write to the same word twice.
	/// `offset` and `bytes.len()` must both be multiples of `WRITE_SIZE`.
	fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error>;
}

/// Marker trait for NorFlash relaxing the restrictions on `write`.
///
/// Writes to the same word twice are now allowed. The result is the logical AND of the
/// previous data and the written data. That is, it is only possible to change 1 bits to 0 bits.
///
/// If power is lost during write:
/// - Bits that were 1 on flash and are written to 1 are guaranteed to stay as 1
/// - Bits that were 1 on flash and are written to 0 are undefined
/// - Bits that were 0 on flash are guaranteed to stay as 0
/// - Rest of the bits in the page are guaranteed to be unchanged
pub trait MultiwriteNorFlash: NorFlash {}

struct Page {
	pub start: u32,
	pub size: usize,
}

impl Page {
	fn new(index: u32, size: usize) -> Self {
		Self {
			start: index * size as u32,
			size,
		}
	}

	/// The end address of the page
	const fn end(&self) -> u32 {
		self.start + self.size as u32
	}
}

impl Region for Page {
	/// Checks if an address offset is contained within the page
	fn contains(&self, address: u32) -> bool {
		(self.start <= address) && (self.end() > address)
	}
}

///
pub struct RmwNorFlashStorage<'a, S> {
	storage: S,
	merge_buffer: &'a mut [u8],
}

impl<'a, S> RmwNorFlashStorage<'a, S>
where
	S: NorFlash,
{
	/// Instantiate a new generic `Storage` from a `NorFlash` peripheral
	///
	/// **NOTE** This will panic if the provided merge buffer,
	/// is smaller than the erase size of the flash peripheral
	pub fn new(nor_flash: S, merge_buffer: &'a mut [u8]) -> Self {
		if merge_buffer.len() < S::ERASE_SIZE {
			panic!("Merge buffer is too small");
		}

		Self {
			storage: nor_flash,
			merge_buffer,
		}
	}
}

impl<'a, S> ReadStorage for RmwNorFlashStorage<'a, S>
where
	S: ReadNorFlash,
{
	type Error = S::Error;

	fn try_read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
		// Nothing special to be done for reads
		self.storage.try_read(offset, bytes)
	}

	fn capacity(&self) -> usize {
		self.storage.capacity()
	}
}

impl<'a, S> Storage for RmwNorFlashStorage<'a, S>
where
	S: NorFlash,
{
	fn try_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
		// Perform read/modify/write operations on the byte slice.
		let last_page = self.storage.capacity() / S::ERASE_SIZE;

		// `data` is the part of `bytes` contained within `page`,
		// and `addr` in the address offset of `page` + any offset into the page as requested by `address`
		for (data, page, addr) in (0..last_page as u32)
			.map(move |i| Page::new(i, S::ERASE_SIZE))
			.overlaps(bytes, offset)
		{
			let offset_into_page = addr.saturating_sub(page.start) as usize;

			self.storage
				.try_read(page.start, &mut self.merge_buffer[..S::ERASE_SIZE])?;

			// If we cannot write multiple times to the same page, we will have to erase it
			self.storage.try_erase(page.start, page.end())?;
			self.merge_buffer[..S::ERASE_SIZE]
				.iter_mut()
				.skip(offset_into_page)
				.zip(data)
				.for_each(|(byte, input)| *byte = *input);
			self.storage
				.try_write(page.start, &self.merge_buffer[..S::ERASE_SIZE])?;
		}
		Ok(())
	}
}

///
pub struct RmwMultiwriteNorFlashStorage<'a, S> {
	storage: S,
	merge_buffer: &'a mut [u8],
}

impl<'a, S> RmwMultiwriteNorFlashStorage<'a, S>
where
	S: MultiwriteNorFlash,
{
	/// Instantiate a new generic `Storage` from a `NorFlash` peripheral
	///
	/// **NOTE** This will panic if the provided merge buffer,
	/// is smaller than the erase size of the flash peripheral
	pub fn new(nor_flash: S, merge_buffer: &'a mut [u8]) -> Self {
		if merge_buffer.len() < S::ERASE_SIZE {
			panic!("Merge buffer is too small");
		}

		Self {
			storage: nor_flash,
			merge_buffer,
		}
	}
}

impl<'a, S> ReadStorage for RmwMultiwriteNorFlashStorage<'a, S>
where
	S: ReadNorFlash,
{
	type Error = S::Error;

	fn try_read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
		// Nothing special to be done for reads
		self.storage.try_read(offset, bytes)
	}

	fn capacity(&self) -> usize {
		self.storage.capacity()
	}
}

impl<'a, S> Storage for RmwMultiwriteNorFlashStorage<'a, S>
where
	S: MultiwriteNorFlash,
{
	fn try_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
		// Perform read/modify/write operations on the byte slice.
		let last_page = self.storage.capacity() / S::ERASE_SIZE;

		// `data` is the part of `bytes` contained within `page`,
		// and `addr` in the address offset of `page` + any offset into the page as requested by `address`
		for (data, page, addr) in (0..last_page as u32)
			.map(move |i| Page::new(i, S::ERASE_SIZE))
			.overlaps(bytes, offset)
		{
			let offset_into_page = addr.saturating_sub(page.start) as usize;

			self.storage
				.try_read(page.start, &mut self.merge_buffer[..S::ERASE_SIZE])?;

			let rhs = &self.merge_buffer[offset_into_page..S::ERASE_SIZE];
			let is_subset = data.iter().zip(rhs.iter()).all(|(a, b)| *a & *b == *a);

			// Check if we can write the data block directly, under the limitations imposed by NorFlash:
			// - We can only change 1's to 0's
			if is_subset {
				// Use `merge_buffer` as allocation for padding `data` to `WRITE_SIZE`
				let offset = addr as usize % S::WRITE_SIZE;
				let aligned_end = data.len() % S::WRITE_SIZE + offset + data.len();
				self.merge_buffer[..aligned_end].fill(0xff);
				self.merge_buffer[offset..offset + data.len()].copy_from_slice(data);
				self.storage
					.try_write(addr - offset as u32, &self.merge_buffer[..aligned_end])?;
			} else {
				self.storage.try_erase(page.start, page.end())?;
				self.merge_buffer[..S::ERASE_SIZE]
					.iter_mut()
					.skip(offset_into_page)
					.zip(data)
					.for_each(|(byte, input)| *byte = *input);
				self.storage
					.try_write(page.start, &self.merge_buffer[..S::ERASE_SIZE])?;
			}
		}
		Ok(())
	}
}
