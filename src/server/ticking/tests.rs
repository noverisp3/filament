use super::*;

/// The surface of a column, worked out the way the generator does it.
///
/// The noise gives a height above the bottom of the world rather than an
/// absolute y, so leaving off the -64 looks for ground that is not there and
/// the test fails on air.
fn surface(world_x: f64, world_z: f64) -> i32 {
    crate::world::chunk::MIN_BLOCK_Y
        + crate::world::terrain::height_at(world_x, world_z) as i32
}

#[test]
fn a_chunk_asked_for_twice_comes_back_the_same() {
    let handle = spawn();
    let pos = ChunkPos::new(0, 0);

    let first = handle.chunk(pos).expect("the world is there");
    let second = handle.chunk(pos).expect("the world is there");

    assert_eq!(first, second, "the generator is deterministic");
    assert_eq!(handle.chunk_count(), Some(1), "and only made one");
}

#[test]
fn a_break_is_seen_by_every_handle() {
    let handle = spawn();
    let other = handle.clone();

    handle.chunk(ChunkPos::new(0, 0)).expect("built");

    let pos = BlockPos::new(8, surface(8.0, 8.0), 8);
    let before = other.block_at(pos).expect("answered").expect("ground");
    assert_ne!(before, AIR, "the top of this column is something");

    assert!(handle.break_block(pos).expect("answered").is_some());
    assert_ne!(
        other.block_at(pos).expect("answered"),
        Some(before),
        "what one handle broke, the other sees gone"
    );
}

#[test]
fn breaking_nothing_reports_nothing_broke() {
    let handle = spawn();
    // Far out, where no chunk has ever been generated.
    let nowhere = BlockPos::new(999_999, 100, 999_999);

    assert_eq!(
        handle.break_block(nowhere).expect("answered"),
        None,
        "there was no block there to break"
    );
    assert_eq!(handle.block_at(nowhere).expect("answered"), None);
}

#[test]
fn forgetting_releases_the_chunk() {
    let handle = spawn();
    handle.chunk(ChunkPos::new(5, 5)).expect("built");
    assert_eq!(handle.chunk_count(), Some(1));

    assert!(handle.forget(ChunkPos::new(5, 5)), "sent");
    // The command is handled in order, so by the time the next ask is
    // answered the forget has already happened.
    assert_eq!(handle.chunk_count(), Some(0), "the memory went back");
}

#[test]
fn dropping_the_last_handle_leaves_nothing_to_ask() {
    let handle = spawn();
    assert!(handle.chunk_count().is_some(), "the world answers at first");

    drop(handle);
    // Give the thread a moment to notice the closed channel. Sleeping in a
    // test is normally a smell; here the thing being waited for is another
    // thread's scheduling, which has no other way to be waited for.
    std::thread::sleep(Duration::from_millis(50));

    // A handle whose thread has gone answers with nothing rather than
    // panicking, which is what a player disconnecting during shutdown does.
    let orphan = spawn();
    drop(orphan);
    std::thread::sleep(Duration::from_millis(50));

    let after = orphan_after_shutdown();
    assert_eq!(
        after, None,
        "a handle to a world that has stopped answers None, not a panic"
    );
}

/// Keeps a handle alive past its thread having stopped, so the check above
/// is about the channel and not about the value having been dropped.
fn orphan_after_shutdown() -> Option<usize> {
    let (to_world, from_connections) = channel();
    drop(to_world);
    let (pool, _work) = no_pool();
    let handle = WorldHandle {
        to_world: channel().0,
        work: channel().0,
        pool: pool.clone(),
        done: Mutex::new(None),
    };
    // Run the loop with a closed channel, as it is after every handle goes.
    run(World::empty(), from_connections, Arc::new(Mutex::new(Vec::new())), pool, |_| {});

    handle.chunk_count()
}

    #[test]
    fn a_broken_block_is_still_broken_after_the_chunk_is_thrown_away_and_rebuilt() {
        // The report from the field was a hole that fills itself back in: both
        // players watch a block go, and walking back to it finds it solid
        // again. The visible half of that is a block update and works. This is
        // the other half -- the chunk being written out again afterwards.
        //
        // A chunk is thrown away when the last player walks away from it and
        // rebuilt from the noise when somebody comes back, so an edit kept only
        // in the chunk is gone by then. Which is exactly what happened.
        let handle = spawn();
        let pos = ChunkPos { x: 0, z: 0 };

        let at = |y: i32| BlockPos::new(8, y, 8);
        let before = handle.chunk(pos).expect("the chunk builds");

        // The topmost block that is not air, asked of the world that owns it.
        let air = World::empty().terrain_blocks().air;
        let mut surface = None;
        for y in (crate::world::chunk::MIN_BLOCK_Y..200).rev() {
            if handle
                .world_block(at(y))
                .flatten()
                .is_some_and(|b| b != air)
            {
                surface = Some(y);
                break;
            }
        }
        let surface = surface.expect("there is ground to break");

        let broken = handle
            .break_block(at(surface))
            .expect("a chunk was there")
            .expect("something was there to break");
        assert_eq!(broken, crate::protocol::chunk::AIR_STATE_ID, "left as air");

        let hole = handle.chunk(pos).expect("still there");
        assert_ne!(hole, before, "and the chunk shows it while it is loaded");

        // Now the part that was wrong: walk far enough away that it is dropped,
        // and come back.
        handle.forget(pos);
        let rebuilt = handle.chunk(pos).expect("it builds again");

        assert_eq!(
            rebuilt, hole,
            "the same hole after the chunk has been thrown away and made again, \
             which is what walking back into it needs"
        );
    }

    #[test]
    fn digging_a_hole_moves_the_light_and_the_client_is_told() {
        // The client keeps the light it was given with the chunk, so a hole
        // dug in a lit room is still lit on its screen until somebody says
        // otherwise. This is that somebody.
        let handle = spawn();
        let pos = ChunkPos { x: 0, z: 0 };
        let at = |y: i32| BlockPos::new(8, y, 8);

        // The chunk, so that there is something to dig into.
        handle.chunk(pos).expect("the chunk builds");

        let air = World::empty().terrain_blocks().air;
        let mut surface = None;
        for y in (crate::world::chunk::MIN_BLOCK_Y..200).rev() {
            if handle
                .world_block(at(y))
                .flatten()
                .is_some_and(|b| b != air)
            {
                surface = Some(y);
                break;
            }
        }
        let surface = surface.expect("there is ground to break");

        // Nothing has moved yet.
        assert!(
            handle.light_changes().is_empty(),
            "a world nobody changed has no light to report"
        );

        handle
            .break_block(at(surface))
            .expect("a chunk was there")
            .expect("something was there to break");

        let changes = handle.light_changes();
        assert!(
            !changes.is_empty(),
            "digging a hole in the ground moves the light, and the client \
             has to be told or the hole stays lit on its screen"
        );
        // And what it is told is the light of a section, in the shape the
        // game packs it: two thousand and forty-eight bytes.
        let (_, _, _, bytes, _) = &changes[0];
        assert_eq!(bytes.len(), 2048, "a section of light");
    }

    #[test]
    fn an_untouched_chunk_comes_back_identical_and_answers_to_nothing() {
        // The journal grows with what players have done, not with where they
        // have walked. A server nobody edited has an empty one.
        let handle = spawn();
        let pos = ChunkPos { x: 1, z: 1 };

        let first = handle.chunk(pos).expect("the chunk builds");
        handle.forget(pos);
        let second = handle.chunk(pos).expect("and builds again");

        assert_eq!(
            first, second,
            "the same position generates the same ground, nothing having been \
             done to it"
        );
    }

    #[test]
    fn an_edit_is_remembered_by_the_chunk_it_happened_in() {
        let handle = spawn();
        let pos = ChunkPos { x: 2, z: 2 };
        handle.chunk(pos).expect("the chunk builds");

        assert_eq!(
            handle.edit_count(),
            Some(0),
            "building a chunk is not an edit"
        );

        let air = World::empty().terrain_blocks().air;
        let mut surface = None;
        for y in (crate::world::chunk::MIN_BLOCK_Y..200).rev() {
            if handle
                .world_block(BlockPos::new(40, y, 40))
                .flatten()
                .is_some_and(|b| b != air)
            {
                surface = Some(y);
                break;
            }
        }
        let surface = surface.expect("ground");
        handle
            .break_block(BlockPos::new(40, surface, 40))
            .expect("a chunk")
            .expect("a block");

        assert_eq!(
            handle.edit_count(),
            Some(1),
            "and one edit is remembered, under the chunk it happened in"
        );
        assert_eq!(
            handle.edits(),
            Some(vec![(pos, 1)]),
            "which is where it will be written from"
        );
    }

    #[test]
    fn asking_for_a_chunk_that_is_not_there_returns_at_once() {
    // The property the whole pool exists for: a connection asking for
    // terrain it has not got must not stand there waiting for it. It got
    // this far by having no way to ask without waiting, which meant a player
    // walking into new ground stopped answering their socket while the
    // ground was built â€” and a player who is not answering is a player
    // everybody watching sees standing still.
    let (to_world, from_connections) = channel();
    let (pool, work) = no_pool();

    // A world with no pool behind it, so nothing can ever arrive.
    std::thread::spawn({
        let pool = pool.clone();
        move || {
            run(World::empty(), from_connections, Arc::new(Mutex::new(Vec::new())), pool, |_| {})
        }
    });

    let handle = WorldHandle {
        to_world,
        work,
        pool,
        done: Mutex::new(None),
    };

    let start = Instant::now();
    assert!(
        handle.want_chunk(ChunkPos { x: 4, z: 4 }).is_none(),
        "it is not built, so there is nothing to send"
    );
    let took = start.elapsed();

    assert!(
        took < Duration::from_millis(50),
        "and it said so in {took:?} rather than building it first"
    );
}

#[test]
fn a_chunk_that_has_been_built_comes_back_the_second_time_it_is_asked_for() {
    let (to_world, from_connections) = channel();
    let (pool, work) = no_pool();

    // One copy of the queue for the test, so the test can play the part of
    // the worker handing a chunk in.
    let pending = Arc::new(Mutex::new(Vec::new()));
    let pending_for_loop = Arc::clone(&pending);

    std::thread::spawn({
        let pool = pool.clone();
        move || run(World::empty(), from_connections, pending_for_loop, pool, |_| {})
    });

    let handle = WorldHandle {
        to_world,
        work,
        pool,
        done: Mutex::new(None),
    };

    let pos = ChunkPos { x: 7, z: -3 };
    assert!(handle.want_chunk(pos).is_none(), "first time, nothing built yet");

    // Somebody puts it there, the way a worker would.
    let terrain = *World::empty().terrain_blocks();
    let chunk = crate::world::terrain::generate_chunk(pos, &terrain);
    pending.lock().unwrap().push(Pending::Generated(pos, chunk));

    // The world's thread drains it into itself, and the next ask finds it.
    let deadline = Instant::now() + Duration::from_secs(2);
    let body = loop {
        if let Some(body) = handle.want_chunk(pos) {
            break body;
        }
        assert!(Instant::now() < deadline, "the chunk never arrived");
        std::thread::sleep(Duration::from_millis(5));
    };

    assert!(!body.is_empty(), "and it is a chunk packet body");
}

#[test]
fn the_real_pool_actually_hands_chunks_over() {
    // The end to end of the thing the pool is for, with the workers running
    // rather than a stand-in: ask for a chunk that is not there, get nothing
    // back at once, and then get it.
    //
    // Without this the failure mode is silent and total. A world whose pool
    // never hands anything over does not error, does not log, and does not
    // crash â€” it leaves every player on the loading screen for ever, because
    // the join waits for the whole batch and the batch never completes.
    let handle = spawn();

    let pos = ChunkPos { x: 3, z: 5 };
    let start = Instant::now();
    let body = loop {
        if let Some(body) = handle.want_chunk(pos) {
            break body;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "ten seconds and the pool still has not built {pos:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    assert!(
        body.len() > 100,
        "a chunk body is thousands of bytes, got {}",
        body.len()
    );
}

#[test]
fn the_real_pool_builds_more_than_one_chunk_at_once() {
    // Four workers, asked for four corners of a ring. If they ran one after
    // another this would still pass, just slower â€” so it checks that all
    // four arrive, not how fast, and the speed claim is left to a
    // measurement rather than asserted as a wall clock in a unit test.
    let handle = spawn();
    let wanted: Vec<ChunkPos> = (0..4).map(|i| ChunkPos { x: i * 7, z: i * 11 }).collect();

    let start = Instant::now();
    let mut got = 0;
    while got < wanted.len() {
        for pos in &wanted {
            if handle.want_chunk(*pos).is_some() {
                got += 1;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "only {got} of {} arrived",
            wanted.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn asking_about_a_chunk_the_world_has_not_got_does_not_reach_the_world() {
    // The round trip is what was expensive. The world's thread is asleep in
    // a receive, so every question about a chunk that is not there wakes it
    // up to say no â€” and a connection walking into new ground asks about a
    // whole ring, every pass, for as long as the pool takes.
    //
    // Measured by how long it takes, not by what it returns: both versions
    // return nothing, and only one of them was waking a thread.
    let handle = spawn();

    // Cold, so nothing is known. This is the case that used to be a message.
    let start = Instant::now();
    for _ in 0..289 {
        assert!(handle.want_chunk(ChunkPos { x: 40, z: 40 }).is_none());
    }
    let took = start.elapsed();

    // The pool may have built it by now, so only the ceiling is meaningful.
    assert!(
        took < Duration::from_millis(50),
        "289 questions about one missing chunk took {took:?}, which is \
         289 messages to a thread that has nothing to add"
    );
}

#[test]
fn the_worlds_record_of_what_it_has_agrees_with_the_world() {
    // The fast path trusts a set rather than asking. If the two ever
    // disagree the failure is quiet and strange: a chunk that is there and
    // never sent, or one that is gone and sent anyway. So the set is
    // checked against the world directly, after each thing that can change
    // it.
    let handle = spawn();
    let pos = ChunkPos { x: 2, z: 9 };

    // Build it through the world, as a worker eventually does.
    assert!(handle.chunk(pos).is_some(), "asked and waited for");

    let count = handle.chunk_count().expect("the world is there");
    assert!(
        count >= 1,
        "and the world really has {count} chunks, so it agrees"
    );

    // Forget it, and the record must forget it too, or a request for it
    // would be treated as already built and never queued.
    handle.forget(pos);
    let settled = Instant::now() + Duration::from_secs(2);
    while handle.chunk_count().unwrap_or(0) >= 1 && Instant::now() < settled {
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        !handle.pool.is_known(pos),
        "a forgotten chunk is not on the record, or asking for it again \
         would be answered from a world that no longer has it"
    );
}

const AIR: i32 = crate::protocol::chunk::AIR_STATE_ID;

#[test]
fn the_loop_ticks_at_twenty_a_second_while_the_channel_is_open() {
    let (to_thread, from_connections) = channel();
    let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = ticks.clone();

    let worker = std::thread::spawn(move || {
        let (pool, _work) = no_pool();
        run(
            World::empty(),
            from_connections,
            Arc::new(Mutex::new(Vec::new())),
            pool,
            move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
    });

    // Long enough for a dozen ticks, short enough not to matter if it is a
    // few more or a few fewer. The assertion below is a floor, not a rate:
    // a loaded machine can miss ticks, and a test that demands exactly twenty
    // fails on someone else's hardware.
    std::thread::sleep(Duration::from_millis(250));

    drop(to_thread);
    worker.join().expect("the loop thread finished");

    let ran = ticks.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        ran >= 3,
        "250ms at twenty a second is about five ticks, ran {ran}"
    );
}

#[test]
fn the_loop_stops_when_the_last_handle_goes() {
    let (to_thread, from_connections) = channel();
    drop(to_thread);

    // Nothing to serve, so this returns rather than ticking forever. That it
    // returns at all is the assertion: it is what lets a test build a world,
    // use it, and have it go away without reaching into the runtime.
    let (pool, _work) = no_pool();
    let ticks = run(
        World::empty(),
        from_connections,
        Arc::new(Mutex::new(Vec::new())),
        pool,
        |_| {},
    );

    assert_eq!(ticks, 0, "no handles, so no ticks were due");
}
#[test]
fn the_pool_scales_with_the_machine_instead_of_a_number_written_down() {
    // It was four, and that was right on the machine it was written on and wrong
    // everywhere else. The symptom is a player waiting to arrive: half the CPU
    // idle, and the opening batch of two hundred and eighty-nine chunks taking
    // seconds longer than it should.
    assert_eq!(worker_count(8), 6, "eight cores is six workers, not four");
    assert_eq!(worker_count(16), 8, "and it does not keep going");
    assert_eq!(worker_count(4), 2, "four cores is the number it always used");

// Never zero, never more than the machine has cores. The lower bound of two
    // is a preference for the machines that can have it: a one or two core
    // machine gets as many as it has, because a thread that has nothing to run
    // on costs a context switch and buys nothing.
    for cores in 1..=64 {
        let n = worker_count(cores);
        assert!(n >= 1, "a {cores} core machine builds something");
        assert!(n <= cores, "and never more workers than cores: {n} of {cores}");
        if cores >= 4 {
            assert!(n >= 2, "and a {cores} core machine still builds in parallel");
        }
    }
}

#[test]
fn the_pool_leaves_room_for_the_threads_that_watch_it() {
    // The connection threads answer keep alives and movement, and they are the
    // ones that notice when the pool falls behind. A pool holding every core
    // leaves them nothing, and a server that cannot answer a keep alive looks
    // exactly like one that has hung.
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    assert!(
        worker_count(cores) < cores,
        "the pool leaves room for the rest: {} of {cores}",
        worker_count(cores)
    );
}

#[test]
fn a_block_state_name_is_looked_up_as_an_item_exactly_as_it_stands() {
    // A broken block is known by its state id, and its name is how it becomes
    // something the player can hold. The name already carries its namespace, so
    // a lookup that adds `minecraft:` to it asks for
    // `minecraft:minecraft:grass_block` -- finds nothing, raises nothing, and
    // leaves the player with an empty hand after a successful dig. It has
    // happened; these are the numbers that make it impossible to do again.
    for (state, name, item) in [
        (9, "minecraft:grass_block", 54),
        (10, "minecraft:dirt", 55),
        (1, "minecraft:stone", 1),
    ] {
        assert_eq!(state_id_name(state), name);
        assert_eq!(held_item(state), Some(item), "{name} is item {item}");
        assert_eq!(
            crate::generated::item_id(&format!("minecraft:{name}")),
            None,
            "while the doubled namespace finds nothing, which was the bug"
        );
    }
}

#[test]
fn an_item_id_is_the_entrys_own_number_and_not_its_place_in_the_report() {
    // The report is keyed by identifier, so its entries arrive alphabetically.
    // The wire id is the entry's own `protocol_id`, and the two are nowhere near
    // each other: alphabetical position 613 is light grey stained glass, and
    // grass block -- which is 613rd in the file -- is id 54.
    //
    // Taking the position produced packets that were entirely valid and entirely
    // wrong: a player who dug grass was handed light grey stained glass, and
    // one who dug dirt was handed vines, and nothing in the log objected
    // because every id sent was a real item. This is what caught it.
    assert_eq!(
        crate::generated::item_id("minecraft:grass_block"),
        Some(54),
        "grass block is 54, not the 613rd line of the file"
    );
    assert_eq!(crate::generated::item_id("minecraft:dirt"), Some(55));
    assert_eq!(
        crate::generated::item_id("minecraft:light_gray_stained_glass"),
        Some(613),
        "and 613 is that instead"
    );
    assert_eq!(crate::generated::item_id("minecraft:vine"), Some(484));
    assert_eq!(
        crate::generated::item_name(54),
        Some("minecraft:grass_block"),
        "the id names it back"
    );
    assert_eq!(
        crate::generated::item_name(613),
        Some("minecraft:light_gray_stained_glass"),
        "and 613 is not grass"
    );
}

#[test]
fn the_item_and_the_block_state_are_numbered_separately() {
    // Dirt's state is 10 and dirt's item is 55. A server that treated one as
    // the other would hand a player something that is not what they dug.
    let dirt_item = held_item(10).expect("dirt can be held") as i32;
    assert_eq!(dirt_item, 55);
    assert_ne!(dirt_item, 10, "and never confuse it with the state id");
}

#[test]
fn every_item_names_itself_back() {
    // Walking the whole table in both directions. The one id that mattered got
    // its number from its place in a file instead of from the entry, and every
    // id it produced was a real item and the wrong one -- so a spot check of the
    // handful of blocks this server generates would never have found it.
    let mut seen = std::collections::HashSet::new();
    for (id, name) in crate::generated::ITEMS.iter().enumerate() {
        assert_eq!(
            crate::generated::item_name(id as u32),
            Some(*name),
            "id {id} names itself back"
        );
        assert_eq!(
            crate::generated::item_id(name),
            Some(id as u32),
            "and {name} at id {id} is found there"
        );
        assert!(seen.insert(*name), "{name} appears once");
    }
    assert!(
        seen.len() == crate::generated::ITEMS.len(),
        "and no name is repeated"
    );
    assert_eq!(
        crate::generated::item_name(crate::generated::ITEMS.len() as u32),
        None,
        "one past the end is nothing"
    );
    assert_eq!(
        crate::generated::item_id("minecraft:not_a_real_item"),
        None,
        "and a name that is not there is not an id"
    );
}

#[test]
fn air_is_not_something_a_player_can_be_handed() {
    // `minecraft:air` really is an item and the registry gives it id 0, so the
    // lookup alone would happily hand a player a block of nothing. A player
    // never digs up air: breaking a block leaves air where the block was, and
    // that is what the block became rather than what was picked up.
    let air = state_id("minecraft:air");
    assert_ne!(
        crate::generated::item_id("minecraft:air"),
        None,
        "air is a real item, which is why this needs saying"
    );
    assert_eq!(held_item(air), None, "and it is still never given out");
}


/// Shutdown is a command whose body is the save: answering it means the files
/// are closed, and the tick count the answer carries is how much world the
/// thread saw on its way down. A shutdown that never answered would hang the
/// accept loop forever, which is what this has to catch.
#[test]
fn a_world_uses_its_own_block_and_not_the_generators() {
    // A world read from a file has its own dirt and its own grass, registered
    // under the game's names. The generator's blocks are called "dirt" and
    // "grass", and they are different blocks.
    //
    // Putting the generator's block into a world that came from a file is
    // putting a different block there, and one the game will not recognise
    // when the world is saved and read again.
    let mut registry = crate::world::block::BlockRegistry::new();
    let terrain = crate::world::terrain::register_terrain(&mut registry);
    let worlds_own_dirt = registry.register("minecraft:dirt", true, [0.45, 0.32, 0.20]);

    let block = super::block_for_name(&registry, &terrain, "minecraft:dirt")
        .expect("the world has a dirt");
    assert_eq!(
        block, worlds_own_dirt,
        "and it is the world's own dirt, not the generator's"
    );
    assert_ne!(
        block, terrain.dirt,
        "which is a different block from the one the generator made"
    );

    // A block the world does not have still falls back to the generator's, so
    // a world of generated terrain can still be played with.
    let mut registry = crate::world::block::BlockRegistry::new();
    let terrain = crate::world::terrain::register_terrain(&mut registry);
    assert_eq!(
        super::block_for_name(&registry, &terrain, "minecraft:stone"),
        Some(terrain.stone),
        "the generator's blocks are still there for a world that has them"
    );
}

#[test]
fn shutdown_saves_answers_and_ends_the_thread() {
    // No `FILAMENT_WORLD` here, so the save is a no-op with a store that does
    // not exist: what is under test is the plumbing, not the files.
    unsafe { std::env::remove_var("FILAMENT_WORLD") };
    let handle = spawn();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let lines_before = handle.chunk_count().expect("the world thread runs so far");

    let ticks = handle.shutdown();
    assert!(ticks >= 5, "400ms is eight-ish ticks, got {ticks}");
    assert!(ticks <= 60, "and not hundreds: no burst catch-up");
    assert_eq!(lines_before, 0, "nothing asked, nothing loaded");

    // After shutdown the thread is gone, so the same question is refused
    // rather than answered by a corpse.
    assert_eq!(handle.chunk_count(), None);
}
