# J08: no change justified

J08 is optional and remains unmodified. Both task-slot loops already reuse four `String` scratch buffers for slot, icon, label, and line IDs, avoiding a fresh ID allocation per slot (`projection.rs:350-361`, `async_projection.rs:665-678`). Both use the shared task visual-state model and task-progress projector.

The icon mechanisms are intentionally different: the synchronous path resolves and installs an icon immediately (`projection.rs:376-377`); the async path schedules visible work through its cache, generation, and binding flow (`async_projection.rs:693-702`, `async_projection.rs:398-430`). A common full-loop helper would need callback scaffolding to retain those strategies, while splitting the loops around icon operations would add calls and could change document mutation order. Either choice adds complexity without reducing the existing ID allocation cost.

The shared label, line, background, and visibility operations are small and currently ordered around each path's icon operation (`projection.rs:378-405`, `async_projection.rs:685-716`). Extracting that tail alone would add parameters/helper surface without eliminating meaningful work. Keep both loops; do not consolidate the distinct sync resolve and async schedule behavior. No before/after cost measurement or Cargo/runtime check was performed because this decision makes no code change.
