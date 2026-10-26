//! Pure virtual-list window model: which rows are visible for a scroll
//! offset, row height, and viewport height. No rendering here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualListModel {
    pub row_count: usize,
    pub row_height: u32,
    pub viewport_height: u32,
    pub scroll_offset: u32,
}

impl VirtualListModel {
    #[must_use]
    pub fn visible_range(self) -> (usize, usize) {
        if self.row_count == 0 || self.row_height == 0 || self.viewport_height == 0 {
            return (0, 0);
        }
        let first = (self.scroll_offset / self.row_height) as usize;
        let first = first.min(self.row_count);
        let rows_visible = self.viewport_height.div_ceil(self.row_height) as usize;
        let last = (first + rows_visible.max(1)).min(self.row_count);
        (first, last)
    }
}
