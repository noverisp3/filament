//! The shape of a player, and the shape of a block.
//!
//! The game measures both in boxes, and it asks about them in two places that
//! look like they are about the same thing and are not:
//!
//! * Whether a player's head is in a wall. The game does not ask whether the
//!   cell the head is in is solid. It asks whether a flat box around the eyes
//!   overlaps the shape of a block that stops a player from breathing, and
//!   that box is most of a block wide, so a player standing beside a wall is
//!   not in it. `Entity.isInWall`.
//! * Whether a block may be placed. The game asks whether the new block's
//!   shape would be inside a player, which is why a block cannot be put where
//!   somebody is standing. `BlockItem.canPlace`, which asks the level whether
//!   the shape is unobstructed.
//!
//! The numbers are the game's own: a player is 0.6 of a block wide and 1.8
//! tall, and crouching makes them 1.5. The eyes are at 1.62.

use super::pos::BlockPos;

/// A player is this wide.
pub const PLAYER_WIDTH: f64 = 0.6;
/// A player standing up is this tall.
pub const STANDING_HEIGHT: f64 = 1.8;
/// A player crouching is this tall.
pub const CROUCHING_HEIGHT: f64 = 1.5;
/// The eyes of a player are this far above their feet.
pub const EYE_HEIGHT: f64 = 1.62;

/// The box the game asks about a wall with.
///
/// It is `0.8` of the player's width wide -- 0.48 of a block -- and a hair
/// thick, so it is a disc lying on its side at the eyes rather than a cube
/// around the head.
const EYE_BOX_WIDTH: f64 = PLAYER_WIDTH * 0.8;
/// How thick the box around the eyes is.
const EYE_BOX_HEIGHT: f64 = 1.0E-6;

/// A box with its corners on the axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box {
    /// The smallest point of the box, on each axis.
    pub min: [f64; 3],
    /// The largest point of the box, on each axis.
    pub max: [f64; 3],
}

impl Box {
    /// A box from its two corners.
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Self {
        Box { min, max }
    }

    /// A box of the given size, centred on a point.
    pub fn centred_at(centre: [f64; 3], size: [f64; 3]) -> Self {
        Box {
            min: [
                centre[0] - size[0] / 2.0,
                centre[1] - size[1] / 2.0,
                centre[2] - size[2] / 2.0,
            ],
            max: [
                centre[0] + size[0] / 2.0,
                centre[1] + size[1] / 2.0,
                centre[2] + size[2] / 2.0,
            ],
        }
    }

    /// Whether this box and another share any space.
    ///
    /// Touching is not sharing: a player standing against a wall has a box
    /// that ends where the wall's begins, and the game does not call that a
    /// collision.
    pub fn intersects(&self, other: &Box) -> bool {
        (0..3).all(|axis| {
            self.min[axis] < other.max[axis] && self.max[axis] > other.min[axis]
        })
    }

    /// Whether this box reaches into the cell at a position.
    pub fn reaches_cell(&self, cell: BlockPos) -> bool {
        let min = [
            cell.x as f64,
            cell.y as f64,
            cell.z as f64,
        ];
        let max = [min[0] + 1.0, min[1] + 1.0, min[2] + 1.0];
        (0..3).all(|axis| self.min[axis] < max[axis] && self.max[axis] > min[axis])
    }

    /// The cells this box reaches into, on the x and z axes.
    ///
    /// The box is flat, so it is in one layer of cells, and it reaches into
    /// the cell next door only when it is close enough to the edge. A player
    /// walking along a wall has their eyes over the wall's cell only near its
    /// edge, and that is the whole difference between being in a wall and
    /// standing beside one.
    pub fn cells(&self) -> Vec<BlockPos> {
        let x0 = self.min[0].floor() as i32;
        let x1 = self.max[0].floor() as i32;
        let z0 = self.min[2].floor() as i32;
        let z1 = self.max[2].floor() as i32;
        // The layer is the one the box is in. A box a hair thick cannot
        // straddle two, and the game would find nothing in the other one.
        let y = ((self.min[1] + self.max[1]) / 2.0).floor() as i32;

        let mut out = Vec::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                out.push(BlockPos::new(x, y, z));
            }
        }
        out
    }
}

/// The box a player's body is in.
///
/// A player is 0.6 of a block wide, so the box is that wide around them, and
/// it is as tall as they are: 1.8 standing and 1.5 crouching.
pub fn player_body(pos: (f64, f64, f64), crouching: bool) -> Box {
    let height = if crouching {
        CROUCHING_HEIGHT
    } else {
        STANDING_HEIGHT
    };
    Box::new(
        [
            pos.0 - PLAYER_WIDTH / 2.0,
            pos.1,
            pos.2 - PLAYER_WIDTH / 2.0,
        ],
        [
            pos.0 + PLAYER_WIDTH / 2.0,
            pos.1 + height,
            pos.2 + PLAYER_WIDTH / 2.0,
        ],
    )
}

/// The flat box around a player's eyes.
///
/// This is the box the game asks about a wall with, and it is a third smaller
/// than the player: 0.48 of a block wide instead of 0.6.
pub fn eye_box(pos: (f64, f64, f64)) -> Box {
    Box::centred_at(
        [pos.0, pos.1 + EYE_HEIGHT, pos.2],
        [EYE_BOX_WIDTH, EYE_BOX_HEIGHT, EYE_BOX_WIDTH],
    )
}

/// The cell a block fills, as a box.
///
/// A block that is a whole cell fills it: stone, wood, wool, glass. A block
/// that is not a whole cell -- a slab, a torch, a flower -- does not, and
/// this server does not know their shapes, so it treats them as filling
/// nothing. The game knows them and would use them, and the difference shows
/// on a slab in a player's head and on a torch in the way.
pub fn filled_cell(cell: BlockPos) -> Box {
    Box::new(
        [cell.x as f64, cell.y as f64, cell.z as f64],
        [
            cell.x as f64 + 1.0,
            cell.y as f64 + 1.0,
            cell.z as f64 + 1.0,
        ],
    )
}

/// Whether a block may be put in a cell, the way the game decides.
///
/// The game asks whether the new block's shape would be inside anybody:
/// `BlockItem.canPlace` asks the level whether the shape is unobstructed, and
/// a player is an entity like any other. So a block cannot be put where
/// somebody is standing, not even where the person putting it is standing.
///
/// A block that is not a whole cell is let through, because its shape is not
/// known here and a torch does not stop a player.
pub fn cell_is_clear(
    cell: BlockPos,
    fills_the_cell: bool,
    players: &[(f64, f64, f64, bool)],
) -> bool {
    if !fills_the_cell {
        return true;
    }
    let shape = filled_cell(cell);
    !players
        .iter()
        .any(|(x, y, z, crouching)| shape.intersects(&player_body((*x, *y, *z), *crouching)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_around_the_eyes_reaches_the_wall_only_near_its_edge() {
        // A player standing in the middle of a cell, with a wall to the
        // east. The eyes are half a block from the wall, and the box around
        // them is a quarter of a block, so it does not reach.
        let eyes = eye_box((8.5, 64.0, 8.5));
        let wall = BlockPos::new(9, 65, 8);

        assert!(!eyes.reaches_cell(wall), "half a block away is not in it");
        assert!(
            eyes.reaches_cell(BlockPos::new(8, 65, 8)),
            "the cell the eyes are in"
        );
    }

    #[test]
    fn the_box_around_the_eyes_reaches_the_wall_when_the_player_is_against_it() {
        // The same player, now with their face against the wall: the eyes
        // are a tenth of a block from it, and the box reaches.
        let eyes = eye_box((8.9, 64.0, 8.5));
        let wall = BlockPos::new(9, 65, 8);

        assert!(eyes.reaches_cell(wall), "a tenth of a block away is in it");
    }

    #[test]
    fn the_cells_the_box_reaches_are_the_ones_it_is_over() {
        let eyes = eye_box((8.5, 64.0, 8.5));
        let cells = eyes.cells();

        assert_eq!(cells.len(), 1, "the eyes are in the middle of a cell");
        assert_eq!(cells[0], BlockPos::new(8, 65, 8));
    }

    #[test]
    fn the_cells_the_box_reaches_include_the_wall_when_the_player_is_against_it() {
        let eyes = eye_box((8.9, 64.0, 8.5));
        let cells = eyes.cells();

        assert_eq!(cells.len(), 2, "the eyes and the wall");
        assert!(cells.contains(&BlockPos::new(8, 65, 8)));
        assert!(cells.contains(&BlockPos::new(9, 65, 8)));
    }

    #[test]
    fn a_players_body_is_the_size_the_game_measures() {
        let standing = player_body((8.5, 64.0, 8.5), false);
        let crouching = player_body((8.5, 64.0, 8.5), true);

        assert!(
            (standing.max[0] - standing.min[0] - PLAYER_WIDTH).abs() < 1.0E-9,
            "the player is 0.6 of a block wide"
        );
        assert!(
            (standing.max[1] - standing.min[1] - STANDING_HEIGHT).abs() < 1.0E-9,
            "and 1.8 tall"
        );
        assert!(
            (crouching.max[1] - crouching.min[1] - CROUCHING_HEIGHT).abs() < 1.0E-9,
            "and 1.5 tall when crouching"
        );
        assert_eq!(standing.min[1], 64.0, "the feet are where the player is");
    }

    #[test]
    fn touching_a_wall_is_not_a_collision() {
        let body = player_body((8.5, 64.0, 8.5), false);
        let wall = filled_cell(BlockPos::new(9, 64, 8));

        assert!(
            !body.intersects(&wall),
            "the player is half a block from the wall"
        );
    }

    #[test]
    fn a_block_under_a_player_is_a_collision() {
        let body = player_body((8.5, 64.0, 8.5), false);
        let block = filled_cell(BlockPos::new(8, 64, 8));

        assert!(body.intersects(&block), "the player is standing in it");
    }

    #[test]
    fn a_block_cannot_be_put_where_a_player_is_standing() {
        let cell = BlockPos::new(8, 64, 8);
        // A player standing in the cell.
        let players = [(8.5, 64.0, 8.5, false)];

        assert!(
            !cell_is_clear(cell, true, &players),
            "the player is in the way"
        );
    }

    #[test]
    fn a_block_can_be_put_where_nobody_is_standing() {
        let cell = BlockPos::new(8, 64, 8);
        // A player in the cell next door.
        let players = [(9.5, 64.0, 8.5, false)];

        assert!(cell_is_clear(cell, true, &players), "the cell is empty");
    }

    #[test]
    fn a_block_that_does_not_fill_its_cell_goes_anywhere() {
        let cell = BlockPos::new(8, 64, 8);
        let players = [(8.5, 64.0, 8.5, false)];

        assert!(
            cell_is_clear(cell, false, &players),
            "a torch does not stop a player"
        );
    }
}
