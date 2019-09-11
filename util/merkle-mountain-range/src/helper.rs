pub fn leaf_index_to_pos(index: u64) -> u64 {
    if index == 0 {
        return 0;
    }
    // leaf_count
    let mut leaves = index + 1;
    let mut tree_node_count = 0;
    let mut height = 0u32;
    while leaves > 1 {
        // get heighest peak height
        height = (leaves as f64).log2() as u32;
        // calculate leaves in peak
        let peak_leaves = 1 << height;
        // heighest positon
        let sub_tree_node_count = get_peak_pos_by_height(height) + 1;
        tree_node_count += sub_tree_node_count;
        leaves -= peak_leaves;
    }
    // two leaves can construct a new peak, the only valid number of leaves is 0 or 1.
    debug_assert!(leaves == 0 || leaves == 1, "remain leaves incorrect");
    if leaves == 1 {
        // add one pos for remain leaf
        // equals to `tree_node_count - 1 + 1`
        tree_node_count
    } else {
        let pos = tree_node_count - 1;
        pos - u64::from(height)
    }
}

// TODO optimize
pub fn leaf_index_to_mmr_size(index: u64) -> u64 {
    let mut pos = leaf_index_to_pos(index);
    while pos_height_in_tree(pos + 1) > pos_height_in_tree(pos) {
        pos += 1
    }
    pos + 1
}

pub fn pos_height_in_tree(mut pos: u64) -> u32 {
    pos += 1;
    fn all_ones(num: u64) -> bool {
        num != 0 && num.count_zeros() == num.leading_zeros()
    }
    fn jump_left(pos: u64) -> u64 {
        let bit_length = 64 - pos.leading_zeros();
        let most_significant_bits = 1 << (bit_length - 1);
        pos - (most_significant_bits - 1)
    }

    while !all_ones(pos) {
        pos = jump_left(pos)
    }

    64 - pos.leading_zeros() - 1
}

pub fn parent_offset(height: u32) -> u64 {
    2 << height
}

pub fn sibling_offset(height: u32) -> u64 {
    (2 << height) - 1
}

pub fn get_peaks(mmr_size: u64) -> Vec<u64> {
    let mut pos_s = Vec::new();
    let (mut height, mut pos) = left_peak_height_pos(mmr_size);
    pos_s.push(pos);
    while height > 0 {
        let peak = match get_right_peak(height, pos, mmr_size) {
            Some(peak) => peak,
            None => break,
        };
        height = peak.0;
        pos = peak.1;
        pos_s.push(pos);
    }
    pos_s
}

fn get_right_peak(mut height: u32, mut pos: u64, mmr_size: u64) -> Option<(u32, u64)> {
    // move to right sibling pos
    pos += sibling_offset(height);
    // loop until we find a pos in mmr
    while pos > mmr_size - 1 {
        if height == 0 {
            return None;
        }
        // move to left child
        pos -= parent_offset(height - 1);
        height -= 1;
    }
    Some((height, pos))
}

fn get_peak_pos_by_height(height: u32) -> u64 {
    (1 << (height + 1)) - 2
}

fn left_peak_height_pos(mmr_size: u64) -> (u32, u64) {
    let mut height = 1;
    let mut prev_pos = 0;
    let mut pos = get_peak_pos_by_height(height);
    while pos < mmr_size {
        height += 1;
        prev_pos = pos;
        pos = get_peak_pos_by_height(height);
    }
    (height - 1, prev_pos)
}
