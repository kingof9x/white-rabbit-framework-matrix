# The games on the Games & patterns tab (white_rabbit/games.py): rules, drawing, pause, score screen
# and high scores. No panels or window needed.
import random

import pytest

np = pytest.importorskip("numpy")

from white_rabbit import games
from white_rabbit.games import LEFT, RIGHT, UP, DOWN, ACTION, HEIGHT


def play(game, seconds, held=(), step=1 / 30):
    for _ in range(int(seconds / step)):
        game.update(step, set(held))


@pytest.mark.parametrize("game_class", games.GAMES)
@pytest.mark.parametrize("width", [9, 18])
def test_every_game_draws_the_panels_it_is_played_on(game_class, width):
    game = game_class(width, random.Random(1))
    for key in (LEFT, UP, RIGHT, DOWN, ACTION):
        game.press(key)
        play(game, 0.3)
    grid = game.draw()
    assert grid.shape == (HEIGHT, width)
    assert grid.min() >= 0.0 and grid.max() <= 1.0 and grid.max() > 0
    assert game.title and game.controls and games.GAMES_BY_KEY[game.key] is game_class


def test_snake_grows_when_it_eats_and_dies_at_the_edge():
    snake = games.Snake(18, random.Random(1))
    head = snake.body[0]
    snake.food = (head[0], head[1] - 1)
    snake.move()
    assert snake.score == 1 and len(snake.body) == 4
    snake.press(LEFT)
    for _ in range(20):
        snake.move()
    assert snake.over


def test_snake_ignores_a_turn_straight_back():
    snake = games.Snake(18, random.Random(1))
    snake.press(DOWN)  # it is going up
    snake.move()
    assert snake.direction == (0, -1) and not snake.over


def test_snake_crosses_from_one_panel_to_the_other():
    snake = games.Snake(18, random.Random(1))
    snake.food = (0, 0)
    snake.body = [(7, 10), (7, 11), (7, 12)]
    snake.press(RIGHT)
    for _ in range(3):
        snake.move()
    assert snake.body[0] == (10, 10) and not snake.over


def test_ledris_clears_a_full_line_and_scores():
    ledris = games.Ledris(18, random.Random(1))
    ledris.board[HEIGHT - 1, :] = 1
    ledris.board[HEIGHT - 1, 0] = 0
    ledris.cells, ledris.x, ledris.y = ((0, 0),), 0, 0
    ledris.press(ACTION)  # drop it into the gap
    assert ledris.flash_rows == [HEIGHT - 1] and ledris.score == 1
    play(ledris, 0.5)
    assert not ledris.flash_rows and not ledris.board[HEIGHT - 1].all()


def test_ledris_turns_and_stays_on_the_board():
    ledris = games.Ledris(9, random.Random(1))
    ledris.cells, ledris.x = games.Ledris.normalised(games.Ledris.SHAPES["I"]), 5
    ledris.press(UP)
    ledris.press(UP)
    for _ in range(10):
        ledris.press(RIGHT)
    assert all(0 <= ledris.x + x < 9 for x, _ in ledris.cells)


def test_ledris_shows_the_next_piece_and_score_on_the_right_panel():
    two = games.Ledris(18, random.Random(1)).draw()
    one = games.Ledris(9, random.Random(1)).draw()
    assert two[:, 9:].max() > 0 and one.shape == (HEIGHT, 9)


def test_ledris_ends_when_the_board_is_full():
    ledris = games.Ledris(9, random.Random(1))
    ledris.board[1:, :] = 1
    ledris.board[1:, 0] = 0
    ledris.spawn()
    assert ledris.over


def test_breakout_breaks_bricks_and_loses_balls():
    game = games.Breakout(18, random.Random(1))
    game.press(ACTION)
    assert not game.stuck
    game.vx, game.vy = 0.0, -game.speed()
    play(game, 2.5)
    assert game.score >= 1
    # Move the paddle away and let the ball fall past it three times
    for _ in range(3):
        game.paddle = 0.0 if game.bx > 9 else 15.0
        game.stuck = False
        game.bx, game.by, game.vx, game.vy = 9.0 if game.paddle == 15.0 else 16.0, 30.0, 0.0, 20.0
        play(game, 0.5)
    assert game.over


def test_breakout_paddle_moves_while_a_key_is_held():
    game = games.Breakout(18, random.Random(1))
    start = game.paddle
    play(game, 0.5, held={RIGHT})
    assert game.paddle > start + 3


def test_rabbit_run_catches_carrots_and_ends_on_a_rock():
    game = games.RabbitRun(18, random.Random(1))
    x = int(round(game.x))
    game.carrot_wait = game.rock_wait = 99
    game.carrots = [[x + 1, HEIGHT - 8.0]]
    play(game, 0.6)
    assert game.score == 1 and not game.over
    game.rocks = [[x + 1, HEIGHT - 9.0]]
    play(game, 0.5)
    assert game.over


def test_rabbit_run_gets_faster():
    game = games.RabbitRun(18, random.Random(1))
    slow = game.speed()
    game.clock = 30
    assert game.speed() > slow


def test_session_counts_in_pauses_and_shows_the_score():
    session = games.GameSession(games.Snake(18, random.Random(1)), best=5)
    first = session.frame()
    assert first.max() > 0  # the 3-2-1
    session.update(2.0)
    session.set_paused(True)
    head = session.game.body[0]
    session.update(1.0)
    assert session.game.body[0] == head, "nothing moves while paused"
    assert session.frame().max() == 1.0  # the pause sign
    session.set_paused(False)
    session.game.score = 7
    session.game.over = True
    session.update(0.1)
    assert session.new_best and session.best == 7
    session.update(2.0)
    assert session.finished
    screen = session.frame()
    assert screen[:, :9].max() > 0 and screen[:, 9:].max() > 0


def test_keys_do_nothing_during_the_count_in():
    session = games.GameSession(games.Snake(18, random.Random(1)))
    session.press(LEFT)
    assert not session.game.turns


def test_high_scores_keep_the_best(tmp_path):
    path = str(tmp_path / "settings.json")
    assert games.load_high_scores(path) == {}
    assert games.save_high_score("snake", 12, path)
    assert not games.save_high_score("snake", 8, path)
    assert games.save_high_score("ledris", 3, path)
    assert games.load_high_scores(path) == {"snake": 12, "ledris": 3}


def test_digits_draw_side_by_side_and_stacked():
    grid = np.zeros((HEIGHT, 9))
    games.draw_digits(grid, "18", 0, 0)
    assert grid[:5, :7].sum() > 0
    grid = np.zeros((HEIGHT, 9))
    games.draw_digits(grid, "42", 3, 0, vertical=True)
    assert grid[6:11, 3:6].sum() > 0
