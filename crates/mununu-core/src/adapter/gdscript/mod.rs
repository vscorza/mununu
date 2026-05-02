//! GDScript controller emitter.
//!
//! Emits synthesized controllers as GDScript code suitable for Godot Engine.
//! The output is a `Node`-derived script with an enum FSM, controllable
//! actions as `bool`-returning methods, and uncontrollable events as
//! signal-handler methods.

pub mod emit_controller;
