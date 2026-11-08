//! `_NET_WM_STATE_ABOVE` / `_NET_WM_STATE_BELOW` as a stacking layer.
//!
//! The server stacking order stays authoritative. Raises (focus, map) may push an above-layer
//! frame under a normal one, so after any raise the frames are checked against the layer order
//! Below < Normal < Above and only the misplaced ones are restacked, preserving relative order.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StackLayer {
    Above,
    Below,
}

/// Rank of a frame in the desired bottom-to-top order: Below, then normal, then Above.
fn layer_rank(layer: Option<StackLayer>) -> u8 {
    match layer {
        Some(StackLayer::Below) => 0,
        None => 1,
        Some(StackLayer::Above) => 2,
    }
}

/// True when a bottom-to-top list of `(frame, layer)` violates the layer order.
pub(super) fn layers_misplaced(stack: &[(Window, Option<StackLayer>)]) -> bool {
    stack
        .windows(2)
        .any(|pair| layer_rank(pair[0].1) > layer_rank(pair[1].1))
}

impl<C: Connection> Wm<'_, C> {
    /// Sets or clears `client`'s layer and restores the stacking order.
    pub(super) fn set_stack_layer(
        &mut self,
        client: Window,
        layer: Option<StackLayer>,
    ) -> Result<(), ReplyError> {
        match layer {
            Some(layer) => self.stack_layers.insert(client, layer),
            None => self.stack_layers.remove(&client),
        };
        self.enforce_stack_layers()?;
        self.publish_client_list()
    }

    /// Restacks only the frames that sit outside their layer. Below frames go directly under the
    /// lowest normal or above frame (never under the desktop), Above frames go to the top.
    pub(super) fn enforce_stack_layers(&mut self) -> Result<(), ReplyError> {
        if self.stack_layers.is_empty() {
            return self.raise_reveal_strips();
        }
        let root = self.screen().root;
        let tree = self.conn.query_tree(root)?.reply()?;
        let ordered: Vec<(Window, Option<StackLayer>)> = tree
            .children
            .iter()
            .filter_map(|frame| {
                let client = self.frame_to_client.get(frame)?;
                Some((*frame, self.stack_layers.get(client).copied()))
            })
            .collect();
        if !layers_misplaced(&ordered) {
            return self.raise_reveal_strips();
        }
        let anchor = ordered
            .iter()
            .find(|(_, layer)| *layer != Some(StackLayer::Below))
            .map(|(frame, _)| *frame);
        for (frame, layer) in &ordered {
            match (layer, anchor) {
                (Some(StackLayer::Below), Some(anchor)) => {
                    // Forward order: each lands directly under the anchor, so earlier (lower)
                    // Below frames end up beneath later ones.
                    self.conn.configure_window(
                        *frame,
                        &ConfigureWindowAux::new()
                            .sibling(anchor)
                            .stack_mode(StackMode::BELOW),
                    )?;
                }
                (Some(StackLayer::Above), _) => {
                    self.conn.configure_window(
                        *frame,
                        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                    )?;
                }
                _ => {}
            }
        }
        self.raise_reveal_strips()
    }
}

#[cfg(test)]
mod stack_layer_tests {
    use super::*;

    #[test]
    fn ordered_layers_are_not_misplaced() {
        let stack = [
            (1, Some(StackLayer::Below)),
            (2, None),
            (3, None),
            (4, Some(StackLayer::Above)),
        ];
        assert!(!layers_misplaced(&stack));
    }

    #[test]
    fn normal_above_an_above_frame_is_misplaced() {
        let stack = [(1, Some(StackLayer::Above)), (2, None)];
        assert!(layers_misplaced(&stack));
    }

    #[test]
    fn above_below_a_normal_frame_is_misplaced_and_below_above_normal_is_not() {
        assert!(layers_misplaced(&[(1, None), (2, Some(StackLayer::Below))]));
        assert!(!layers_misplaced(&[
            (1, Some(StackLayer::Below)),
            (2, None)
        ]));
    }

    #[test]
    fn empty_and_single_stacks_are_never_misplaced() {
        assert!(!layers_misplaced(&[]));
        assert!(!layers_misplaced(&[(1, Some(StackLayer::Above))]));
    }
}
