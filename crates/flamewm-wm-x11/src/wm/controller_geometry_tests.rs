    use super::*;

    fn children(base: u32) -> [u32; 12] {
        std::array::from_fn(|i| base + i as u32)
    }

    fn regions() -> [Option<FrameRegion>; 12] {
        std::array::from_fn(|i| (i == 8).then_some(FrameRegion::TitleDrag))
    }

    fn outputs(width: i32) -> [RootRect; 1] {
        [RootRect::new(0, 0, width, 1080)]
    }

    #[test]
    fn unchanged_motions_rebuild_once() {
        let mut cache = ControllerGeometryCache::default();
        for _ in 0..1000 {
            let geometry = cache.fetch(7, children(100), regions(), outputs(1920).into_iter());
            assert_eq!(geometry.region_map.get(&108), Some(&FrameRegion::TitleDrag));
            assert_eq!(geometry.region_map.len(), 1);
            assert_eq!(*geometry.outputs, outputs(1920));
        }
        assert_eq!(cache.rebuilds, 1);
    }

    #[test]
    fn client_region_or_output_change_rebuilds() {
        let mut cache = ControllerGeometryCache::default();
        cache.fetch(7, children(100), regions(), outputs(1920).into_iter());
        cache.fetch(8, children(100), regions(), outputs(1920).into_iter());
        assert_eq!(cache.rebuilds, 2);
        cache.fetch(8, children(200), regions(), outputs(1920).into_iter());
        assert_eq!(cache.rebuilds, 3);
        let mut moved = regions();
        moved[8] = None;
        let geometry = cache.fetch(8, children(200), moved, outputs(1920).into_iter());
        assert!(geometry.region_map.is_empty());
        assert_eq!(cache.rebuilds, 4);
        cache.fetch(8, children(200), moved, outputs(1280).into_iter());
        assert_eq!(cache.rebuilds, 5);
    }
