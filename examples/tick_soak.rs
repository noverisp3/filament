// Two minutes, one world, no players: does the tick queue settle or grow?
//
// The failure this looks for is not a wrong answer but a growing one. A tick
// that schedules something that schedules something, with nothing draining it,
// is quiet right up until it is not -- and it is not visible in any test that
// runs a handful of ticks.

use std::time::{Duration, Instant};

fn main() {
    let mut world = filament::world::world::World::generate(1);
    let terrain = *world.terrain_blocks();
    let pos = filament::world::pos::BlockPos::new(8, 64, 8);

    world.set_block(
        pos.offset(filament::world::pos::Direction::Down),
        terrain.grass,
    );
    world.set_block(pos, terrain.dirt);
    world.set_block(pos.offset(filament::world::pos::Direction::Up), terrain.air);

    let wanted = 2400; // two minutes at twenty a second
    let started = Instant::now();
    let mut slowest = Duration::ZERO;

    for tick in 0..wanted {
        let before = Instant::now();
        world.tick();
        slowest = slowest.max(before.elapsed());
        let took = before.elapsed();

        if tick % 600 == 0 || took > Duration::from_millis(20) {
            println!(
                "tick {:5}  {:>7} ms  game {:5}  day {:5}  scheduled {:4}  pending {:3}",
                tick,
                took.as_millis(),
                world.game_time(),
                world.day_time(),
                world.scheduled_ticks(),
                world.pending_tick_changes()
            );
        }

        // Someone collects the changes, as a connection would.
        let changes = world.take_tick_changes();
        if !changes.is_empty() && tick % 600 == 0 {
            println!("             the world changed {} block(s)", changes.len());
        }
    }

    let elapsed = started.elapsed();
    println!("\n{} ticks in {:?}", wanted, elapsed);
    println!("slowest single tick: {:?}", slowest);
    println!("scheduled left: {}", world.scheduled_ticks());
    println!("pending changes: {}", world.pending_tick_changes());

    // The point of the whole exercise: an idle world costs nothing per tick,
    // forever. A backlog that never drains is a world that eats its tick
    // budget growing, and it grows until the tick stops being fifty
    // milliseconds and starts being a second.
    let per_tick = elapsed / wanted as u32;
    println!("average per tick: {per_tick:?}");
    if per_tick > Duration::from_millis(1) {
        println!("\nA tick is fifty milliseconds. This one is over a millisecond of real work.");
        println!("That is fine for a world doing things. It is not fine if it is also growing.");
    }
    assert!(
        world.scheduled_ticks() < wanted,
        "the queue is not holding more work than it was given"
    );
}
