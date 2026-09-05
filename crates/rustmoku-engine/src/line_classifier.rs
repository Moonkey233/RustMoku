//! Slow, independent Freestyle semantics used by the build script and tests.
//! Every counted five must include the original candidate (cell 4). This
//! excludes unrelated runs elsewhere in the window. Walls/opponents block it.

/// Codes: 0 Quiet, 1 Three, 2 OpenThree, 3 Four, 4 OpenFour, 5 Five.
pub fn classify(key: u16, color: u8) -> u8 {
    let mut cells = [0; 9];
    let mut field = 0;
    for (index, cell) in cells.iter_mut().enumerate() {
        if index == 4 {
            *cell = color;
        } else {
            *cell = ((key >> (field * 2)) & 3) as u8;
            field += 1;
        }
    }
    if five_through_center(&cells, color) {
        return 5;
    }
    match winning_continuations(&mut cells, color) {
        2.. => return 4,
        1 => return 3,
        _ => {}
    }
    let mut forcing = 0;
    let mut open = 0;
    for next in 0..9 {
        if cells[next] != 0 {
            continue;
        }
        cells[next] = color;
        let wins = winning_continuations(&mut cells, color);
        forcing += u8::from(wins >= 1);
        open += u8::from(wins >= 2);
        cells[next] = 0;
    }
    if open > 0 {
        2
    } else if forcing > 0 {
        1
    } else {
        0
    }
}

fn five_through_center(cells: &[u8; 9], color: u8) -> bool {
    (0..=4).any(|start| cells[start..start + 5].iter().all(|&cell| cell == color))
}

fn winning_continuations(cells: &mut [u8; 9], color: u8) -> u8 {
    let mut count = 0;
    for next in 0..9 {
        if cells[next] != 0 {
            continue;
        }
        cells[next] = color;
        count += u8::from(five_through_center(cells, color));
        cells[next] = 0;
    }
    count
}
