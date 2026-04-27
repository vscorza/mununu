extends CharacterBody2D

enum State { IDLE, RUNNING, JUMPING, FALLING, ATTACKING, DEAD }
var current_state: State = State.IDLE

func _physics_process(delta):
	match current_state:
		State.IDLE:
			if Input.is_action_pressed("move"):
				current_state = State.RUNNING
			if Input.is_action_just_pressed("jump"):
				current_state = State.JUMPING
			if Input.is_action_just_pressed("attack"):
				current_state = State.ATTACKING
		State.RUNNING:
			if not Input.is_action_pressed("move"):
				current_state = State.IDLE
			if Input.is_action_just_pressed("jump"):
				current_state = State.JUMPING
		State.JUMPING:
			current_state = State.FALLING
		State.FALLING:
			if is_on_floor():
				current_state = State.IDLE
		State.ATTACKING:
			if animation_finished:
				current_state = State.IDLE

func _on_damage_received():
	current_state = State.DEAD
