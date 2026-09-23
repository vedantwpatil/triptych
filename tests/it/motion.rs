use triptych::app::{Feed, KeyPrefix, Motion, MotionKey, find_match, grid_target, list_target};

fn keys(prefix: &mut KeyPrefix, typed: &str) -> Vec<Feed> {
    typed.chars().map(|c| prefix.feed(MotionKey::Char(c))).collect()
}

const fn motion(motion: Motion, count: Option<usize>) -> Feed {
    Feed::Motion { motion, count }
}

#[test]
fn a_bare_motion_key_has_no_count() {
    let mut p = KeyPrefix::default();
    assert_eq!(keys(&mut p, "j"), [motion(Motion::Down, None)]);
    assert_eq!(keys(&mut p, "G"), [motion(Motion::Bottom, None)]);
}

#[test]
fn digits_build_a_count_and_the_next_motion_uses_it() {
    let mut p = KeyPrefix::default();
    assert_eq!(keys(&mut p, "12j"), [Feed::Consumed, Feed::Consumed, motion(Motion::Down, Some(12))]);
    assert!(!p.is_pending());
}

#[test]
fn zero_is_line_start_alone_and_a_digit_after_a_count() {
    let mut p = KeyPrefix::default();
    assert_eq!(keys(&mut p, "0"), [motion(Motion::LineStart, None)]);
    assert_eq!(keys(&mut p, "10k"), [Feed::Consumed, Feed::Consumed, motion(Motion::Up, Some(10))]);
}

#[test]
fn gg_goes_to_the_top_and_a_count_picks_the_row() {
    let mut p = KeyPrefix::default();
    assert_eq!(keys(&mut p, "gg"), [Feed::Consumed, motion(Motion::Top, None)]);
    assert_eq!(keys(&mut p, "3gg"), [Feed::Consumed, Feed::Consumed, motion(Motion::Top, Some(3))]);
}

#[test]
fn a_key_that_is_not_a_motion_cancels_the_prefix_and_is_swallowed() {
    let mut p = KeyPrefix::default();
    assert_eq!(keys(&mut p, "3d"), [Feed::Consumed, Feed::Consumed]);
    assert!(!p.is_pending());
    assert_eq!(keys(&mut p, "gx"), [Feed::Consumed, Feed::Consumed]);
    assert!(!p.is_pending());
    assert_eq!(keys(&mut p, "d"), [Feed::Other]);
}

#[test]
fn esc_clears_a_pending_prefix_and_only_then() {
    let mut p = KeyPrefix::default();
    assert_eq!(p.feed(MotionKey::Esc), Feed::Other);
    keys(&mut p, "4");
    assert_eq!(p.feed(MotionKey::Esc), Feed::Consumed);
    assert!(!p.is_pending());
}

#[test]
fn ctrl_d_and_ctrl_u_are_half_page_motions_and_take_a_count() {
    let mut p = KeyPrefix::default();
    assert_eq!(p.feed(MotionKey::Ctrl('d')), motion(Motion::HalfDown, None));
    assert_eq!(p.feed(MotionKey::Ctrl('u')), motion(Motion::HalfUp, None));
    assert_eq!(p.feed(MotionKey::Ctrl('x')), Feed::Other);
}

#[test]
fn arrows_map_to_the_same_motions_as_hjkl() {
    let mut p = KeyPrefix::default();
    assert_eq!(p.feed(MotionKey::Down), motion(Motion::Down, None));
    assert_eq!(p.feed(MotionKey::Left), motion(Motion::Left, None));
    keys(&mut p, "2");
    assert_eq!(p.feed(MotionKey::Up), motion(Motion::Up, Some(2)));
}

#[test]
fn a_huge_count_is_clamped_instead_of_overflowing() {
    let mut p = KeyPrefix::default();
    let typed = "9".repeat(40);
    keys(&mut p, &typed);
    assert_eq!(p.feed(MotionKey::Char('j')), motion(Motion::Down, Some(100_000)));
}

#[test]
fn list_target_moves_and_clamps_to_the_list() {
    assert_eq!(list_target(2, 10, 10, Motion::Down, Some(5)), Some(7));
    assert_eq!(list_target(8, 10, 10, Motion::Down, Some(5)), Some(9));
    assert_eq!(list_target(2, 10, 10, Motion::Up, Some(5)), Some(0));
    assert_eq!(list_target(2, 10, 10, Motion::Up, None), Some(1));
}

#[test]
fn list_target_top_and_bottom_take_a_one_based_row() {
    assert_eq!(list_target(5, 10, 10, Motion::Top, None), Some(0));
    assert_eq!(list_target(5, 10, 10, Motion::Bottom, None), Some(9));
    assert_eq!(list_target(5, 10, 10, Motion::Top, Some(3)), Some(2));
    assert_eq!(list_target(5, 10, 10, Motion::Bottom, Some(4)), Some(3));
    assert_eq!(list_target(5, 10, 10, Motion::Bottom, Some(99)), Some(9));
}

#[test]
fn list_target_half_page_is_half_the_visible_rows_and_at_least_one() {
    assert_eq!(list_target(0, 50, 20, Motion::HalfDown, None), Some(10));
    assert_eq!(list_target(15, 50, 20, Motion::HalfUp, None), Some(5));
    assert_eq!(list_target(0, 50, 1, Motion::HalfDown, None), Some(1));
    assert_eq!(list_target(45, 50, 20, Motion::HalfDown, None), Some(49));
}

#[test]
fn list_target_is_none_for_empty_lists_and_sideways_motions() {
    assert_eq!(list_target(0, 0, 10, Motion::Down, None), None);
    assert_eq!(list_target(0, 0, 10, Motion::Bottom, None), None);
    for m in [Motion::Left, Motion::Right, Motion::LineStart, Motion::LineEnd] {
        assert_eq!(list_target(3, 10, 10, m, None), None);
    }
}

#[test]
fn grid_target_moves_in_both_axes_and_clamps() {
    let size = (16, 7);
    assert_eq!(grid_target((2, 3), size, Motion::Down, Some(5)), (7, 3));
    assert_eq!(grid_target((14, 3), size, Motion::Down, Some(5)), (15, 3));
    assert_eq!(grid_target((2, 3), size, Motion::Right, Some(2)), (2, 5));
    assert_eq!(grid_target((2, 3), size, Motion::Right, Some(9)), (2, 6));
    assert_eq!(grid_target((2, 3), size, Motion::Left, Some(9)), (2, 0));
}

#[test]
fn grid_target_line_motions_jump_to_the_first_and_last_day() {
    let size = (16, 7);
    assert_eq!(grid_target((4, 3), size, Motion::LineStart, None), (4, 0));
    assert_eq!(grid_target((4, 3), size, Motion::LineEnd, None), (4, 6));
}

#[test]
fn grid_target_top_bottom_and_half_page_keep_the_day() {
    let size = (16, 7);
    assert_eq!(grid_target((4, 3), size, Motion::Top, None), (0, 3));
    assert_eq!(grid_target((4, 3), size, Motion::Bottom, None), (15, 3));
    assert_eq!(grid_target((4, 3), size, Motion::Top, Some(6)), (5, 3));
    assert_eq!(grid_target((4, 3), size, Motion::HalfDown, None), (12, 3));
    assert_eq!(grid_target((4, 3), size, Motion::HalfUp, None), (0, 3));
}

#[test]
fn find_match_searches_forward_and_wraps() {
    let hit = |i: usize| i == 1 || i == 4;
    assert_eq!(find_match(6, 0, true, hit), Some((1, false)));
    assert_eq!(find_match(6, 1, true, hit), Some((4, false)));
    assert_eq!(find_match(6, 4, true, hit), Some((1, true)));
}

#[test]
fn find_match_searches_backward_and_wraps() {
    let hit = |i: usize| i == 1 || i == 4;
    assert_eq!(find_match(6, 5, false, hit), Some((4, false)));
    assert_eq!(find_match(6, 4, false, hit), Some((1, false)));
    assert_eq!(find_match(6, 1, false, hit), Some((4, true)));
}

#[test]
fn find_match_finds_a_lone_match_under_the_cursor_and_none_when_absent() {
    assert_eq!(find_match(5, 2, true, |i| i == 2), Some((2, true)));
    assert_eq!(find_match(5, 2, false, |i| i == 2), Some((2, true)));
    assert_eq!(find_match(5, 2, true, |_| false), None);
    assert_eq!(find_match(0, 0, true, |_| true), None);
}
