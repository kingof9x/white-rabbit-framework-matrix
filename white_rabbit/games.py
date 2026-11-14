# The games on the Games & patterns tab: Snake, Ledris, Breakout and Rabbit run.
#
# Each game is plain logic with no window or panel code, so it can be tested on its own. It draws
# a 34-row picture as wide as the panels it is played on (9 for one panel, 18 for both) with
# brightness from 0.0 to 1.0. GameSession (below) adds what every game shares: a count-in, pause,
# the score screen at the end and high scores.

# Built In Dependencies
import random

# External Dependencies
import numpy as np

HEIGHT = 34
PANEL_WIDTH = 9

LEFT, RIGHT, UP, DOWN, ACTION = "left", "right", "up", "down", "action"

# 3 x 5 digits for scores
DIGITS = {
    "0": ("111", "101", "101", "101", "111"), "1": ("010", "110", "010", "010", "111"),
    "2": ("111", "001", "111", "100", "111"), "3": ("111", "001", "011", "001", "111"),
    "4": ("101", "101", "111", "001", "001"), "5": ("111", "100", "111", "001", "111"),
    "6": ("111", "100", "111", "101", "111"), "7": ("111", "001", "010", "010", "010"),
    "8": ("111", "101", "111", "101", "111"), "9": ("111", "101", "111", "001", "111"),
}

# The White Rabbit, 5 x 6, for Rabbit run
RABBIT = ("10010", "10010", "11110", "10101", "11110", "01110")


def draw_digits(grid, text, x, y, level=1.0, vertical=False):
    """Write digits into grid (rows x columns), 3 x 5 each, side by side or stacked."""
    for i, ch in enumerate(str(text)):
        glyph = DIGITS.get(ch)
        if glyph is None:
            continue
        gx, gy = (x, y + i * 6) if vertical else (x + i * 4, y)
        for dy, row in enumerate(glyph):
            for dx, c in enumerate(row):
                if c == "1" and 0 <= gy + dy < grid.shape[0] and 0 <= gx + dx < grid.shape[1]:
                    grid[gy + dy, gx + dx] = level


def stamp(grid, sprite, x, y, level=1.0):
    for dy, row in enumerate(sprite):
        for dx, c in enumerate(row):
            if c == "1" and 0 <= y + dy < grid.shape[0] and 0 <= x + dx < grid.shape[1]:
                grid[y + dy, x + dx] = level


class Game:
    """What every game has. update(dt, held) moves it on by dt seconds; held is the set of keys held down."""
    key = ""
    title = ""
    # Follows the "Both panels" choice (side by side, keyboard gap or mirror). Ledris doesn't: its
    # board is the left panel and the next piece is on the right.
    spans = True
    controls = ""

    def __init__(self, width=2 * PANEL_WIDTH, rng=None):
        self.width = width
        self.rng = rng or random.Random()
        self.score = 0
        self.over = False

    def press(self, key):
        pass

    def update(self, dt, held=()):
        pass

    def draw(self):
        return np.zeros((HEIGHT, self.width))


class Snake(Game):
    key = "snake"
    title = "Snake"
    controls = "Arrow keys to steer. Eat the blinking dot; don't hit the edge or yourself."

    def __init__(self, width=2 * PANEL_WIDTH, rng=None):
        super().__init__(width, rng)
        x = width // 2
        self.body = [(x, 20), (x, 21), (x, 22)]
        self.direction = (0, -1)
        self.turns = []
        self.wait = 0.0
        self.clock = 0.0
        self.food = self.place_food()

    def place_food(self):
        free = [(x, y) for x in range(self.width) for y in range(HEIGHT) if (x, y) not in self.body]
        return self.rng.choice(free) if free else None

    def press(self, key):
        step = {LEFT: (-1, 0), RIGHT: (1, 0), UP: (0, -1), DOWN: (0, 1)}.get(key)
        if step is None:
            return
        # Remember up to two quick turns; a turn straight back into the body is ignored
        last = self.turns[-1] if self.turns else self.direction
        if step != last and step != (-last[0], -last[1]) and len(self.turns) < 2:
            self.turns.append(step)

    def tick_seconds(self):
        return max(0.06, 0.16 - self.score * 0.004)

    def update(self, dt, held=()):
        self.clock += dt
        self.wait += dt
        while self.wait >= self.tick_seconds() and not self.over:
            self.wait -= self.tick_seconds()
            self.move()

    def move(self):
        if self.turns:
            self.direction = self.turns.pop(0)
        hx, hy = self.body[0]
        head = (hx + self.direction[0], hy + self.direction[1])
        grows = head == self.food
        body = self.body if grows else self.body[:-1]
        if not (0 <= head[0] < self.width and 0 <= head[1] < HEIGHT) or head in body:
            self.over = True
            return
        self.body = [head] + body
        if grows:
            self.score += 1
            self.food = self.place_food()
            if self.food is None:
                self.over = True

    def draw(self):
        grid = np.zeros((HEIGHT, self.width))
        for i, (x, y) in enumerate(self.body):
            grid[y, x] = 1.0 if i == 0 else max(0.35, 0.8 - i * 0.015)
        if self.food is not None:
            grid[self.food[1], self.food[0]] = 1.0 if (self.clock * 4) % 2 < 1 else 0.45
        return grid


class Ledris(Game):
    """Falling blocks. The board is the left panel; with both panels the right one shows the next
    piece, the score and the level."""
    key = "ledris"
    title = "Ledris"
    spans = False
    controls = "Left and right to move, up to turn, down to drop faster, space to drop at once."

    SHAPES = {
        "I": ((0, 0), (1, 0), (2, 0), (3, 0)), "O": ((0, 0), (1, 0), (0, 1), (1, 1)),
        "T": ((0, 0), (1, 0), (2, 0), (1, 1)), "S": ((1, 0), (2, 0), (0, 1), (1, 1)),
        "Z": ((0, 0), (1, 0), (1, 1), (2, 1)), "L": ((0, 0), (0, 1), (0, 2), (1, 2)),
        "J": ((1, 0), (1, 1), (1, 2), (0, 2)),
    }
    POINTS = (0, 1, 3, 5, 8)
    FLASH_SECONDS = 0.3

    def __init__(self, width=2 * PANEL_WIDTH, rng=None):
        super().__init__(width, rng)
        self.board = np.zeros((HEIGHT, PANEL_WIDTH), dtype=int)
        self.lines = 0
        self.level = 1
        self.next = self.rng.choice(list(self.SHAPES))
        self.fall = 0.0
        self.soft = False
        self.flash_rows = []
        self.flash = 0.0
        self.spawn()

    @staticmethod
    def normalised(cells):
        mx = min(x for x, _ in cells)
        my = min(y for _, y in cells)
        return tuple(sorted((x - mx, y - my) for x, y in cells))

    def spawn(self):
        self.cells = self.normalised(self.SHAPES[self.next])
        self.next = self.rng.choice(list(self.SHAPES))
        w = max(x for x, _ in self.cells) + 1
        self.x, self.y = (PANEL_WIDTH - w) // 2, 0
        if not self.fits(self.cells, self.x, self.y):
            self.over = True

    def fits(self, cells, ox, oy):
        for x, y in cells:
            X, Y = ox + x, oy + y
            if X < 0 or X >= PANEL_WIDTH or Y >= HEIGHT or (Y >= 0 and self.board[Y, X]):
                return False
        return True

    def press(self, key):
        if self.over or self.flash_rows:
            return
        if key in (LEFT, RIGHT):
            dx = -1 if key == LEFT else 1
            if self.fits(self.cells, self.x + dx, self.y):
                self.x += dx
        elif key == UP:
            turned = self.normalised([(-y, x) for x, y in self.cells])
            # Turn, nudging away from a wall if needed
            for dx in (0, -1, 1, -2, 2):
                if self.fits(turned, self.x + dx, self.y):
                    self.cells, self.x = turned, self.x + dx
                    break
        elif key == ACTION:
            while self.fits(self.cells, self.x, self.y + 1):
                self.y += 1
            self.lock()

    def fall_seconds(self):
        return max(0.07, 0.55 - (self.level - 1) * 0.05)

    def update(self, dt, held=()):
        if self.over:
            return
        if self.flash_rows:
            self.flash -= dt
            if self.flash <= 0:
                keep = [r for r in range(HEIGHT) if r not in self.flash_rows]
                self.board = np.vstack([np.zeros((len(self.flash_rows), PANEL_WIDTH), dtype=int), self.board[keep]])
                self.flash_rows = []
                self.spawn()
            return
        self.fall += dt
        step = min(self.fall_seconds(), 0.05) if DOWN in held else self.fall_seconds()
        while self.fall >= step and not self.over and not self.flash_rows:
            self.fall -= step
            if self.fits(self.cells, self.x, self.y + 1):
                self.y += 1
            else:
                self.lock()

    def lock(self):
        for x, y in self.cells:
            if y + self.y < 0:
                self.over = True
                return
            self.board[self.y + y, self.x + x] = 1
        self.fall = 0.0
        full = [r for r in range(HEIGHT) if self.board[r].all()]
        if full:
            self.lines += len(full)
            self.score += self.POINTS[min(4, len(full))]
            self.level = 1 + self.lines // 5
            self.flash_rows = full
            self.flash = self.FLASH_SECONDS
        else:
            self.spawn()

    def draw(self):
        grid = np.zeros((HEIGHT, self.width))
        grid[:, :PANEL_WIDTH] = self.board * 0.7
        if self.flash_rows:
            grid[self.flash_rows, :PANEL_WIDTH] = 1.0 if int(self.flash * 20) % 2 == 0 else 0.25
        elif not self.over:
            for x, y in self.cells:
                if 0 <= self.y + y < HEIGHT:
                    grid[self.y + y, self.x + x] = 1.0
        if self.width >= 2 * PANEL_WIDTH:
            right = PANEL_WIDTH
            grid[0, right:] = 0.2
            grid[7, right:] = 0.2
            for x, y in self.normalised(self.SHAPES[self.next]):
                grid[2 + y, right + 2 + x] = 1.0
            draw_digits(grid, str(min(self.score, 999)).zfill(3), right + 3, 10, 1.0, vertical=True)
            draw_digits(grid, str(min(self.level, 9)), right + 3, 28, 0.5)
        return grid


class Breakout(Game):
    key = "breakout"
    title = "Breakout"
    controls = "Left and right to move the paddle, space to launch the ball. Three balls."

    PADDLE = 3

    def __init__(self, width=2 * PANEL_WIDTH, rng=None):
        super().__init__(width, rng)
        self.level = 1
        self.lives = 3
        self.paddle = (width - self.PADDLE) / 2
        self.move_wait = 0.0
        self.new_bricks()
        self.serve()

    def new_bricks(self):
        self.bricks = np.zeros((HEIGHT, self.width), dtype=int)
        self.bricks[2:8, :] = 1

    def serve(self):
        self.stuck = True
        self.launch_wait = 1.5
        self.vx, self.vy = 0.0, 0.0
        self.follow_paddle()

    def follow_paddle(self):
        self.bx, self.by = self.paddle + self.PADDLE / 2 - 0.5, HEIGHT - 2.0

    def speed(self):
        return 14.0 + 3.0 * (self.level - 1)

    def launch(self):
        self.stuck = False
        direction = self.rng.choice((-1, 1))
        speed = self.speed()
        self.vx, self.vy = direction * speed * 0.45, -speed * 0.9

    def press(self, key):
        if key in (LEFT, RIGHT):
            self.move_paddle(-1 if key == LEFT else 1)
            self.move_wait = 0.12
        elif key == ACTION and self.stuck:
            self.launch()

    def move_paddle(self, dx):
        self.paddle = float(min(max(0, self.paddle + dx), self.width - self.PADDLE))
        if self.stuck:
            self.follow_paddle()

    def update(self, dt, held=()):
        if self.over:
            return
        # Held keys keep the paddle moving, 20 steps a second
        self.move_wait -= dt
        while self.move_wait <= 0 and (LEFT in held) != (RIGHT in held):
            self.move_paddle(-1 if LEFT in held else 1)
            self.move_wait += 0.05
        if self.move_wait < 0:
            self.move_wait = 0.0
        if self.stuck:
            self.launch_wait -= dt
            if self.launch_wait <= 0:
                self.launch()
            return
        # Small steps so the ball can't jump over a brick
        steps = max(1, int(np.ceil(max(abs(self.vx), abs(self.vy)) * dt / 0.4)))
        for _ in range(steps):
            self.move_ball(dt / steps)
            if self.stuck or self.over:
                break

    def move_ball(self, dt):
        self.bx += self.vx * dt
        self.by += self.vy * dt
        if self.bx < 0 or self.bx > self.width - 1:
            self.vx = abs(self.vx) if self.bx < 0 else -abs(self.vx)
            self.bx = min(max(self.bx, 0.0), self.width - 1.0)
        if self.by < 0:
            self.vy = abs(self.vy)
            self.by = 0.0
        col, row = int(round(self.bx)), int(round(self.by))
        if 0 <= row < HEIGHT and self.bricks[row, col]:
            # Bricks are 2 LEDs wide
            left = col - col % 2
            self.bricks[row, left:left + 2] = 0
            self.vy = -self.vy
            self.score += 1
            if not self.bricks.any():
                self.level += 1
                self.new_bricks()
                self.serve()
            return
        if self.vy > 0 and row >= HEIGHT - 1:
            if self.paddle - 0.5 <= self.bx <= self.paddle + self.PADDLE - 0.5:
                # Where the ball hits the paddle sets its angle
                offset = (self.bx - (self.paddle + self.PADDLE / 2 - 0.5)) / (self.PADDLE / 2)
                speed = self.speed()
                self.vx = max(-0.85, min(0.85, offset * 0.7 + (self.rng.random() - 0.5) * 0.2)) * speed
                self.vy = -(speed ** 2 - self.vx ** 2) ** 0.5
                self.by = HEIGHT - 2.0
            elif self.by > HEIGHT:
                self.lives -= 1
                if self.lives <= 0:
                    self.over = True
                else:
                    self.serve()

    def draw(self):
        grid = np.where(np.arange(HEIGHT)[:, None] % 2 == 0, 0.8, 0.5) * self.bricks
        grid[2:8, 1::2] *= 0.75
        p = int(round(self.paddle))
        grid[HEIGHT - 1, p:p + self.PADDLE] = 1.0
        row, col = int(round(self.by)), int(round(self.bx))
        if 0 <= row < HEIGHT:
            grid[row, col] = 1.0
        # Balls left, in the top corner
        for i in range(self.lives - 1):
            grid[0, self.width - 1 - 2 * i] = 0.4
        return grid


class RabbitRun(Game):
    key = "rabbit-run"
    title = "Rabbit run"
    controls = "Left and right to dodge the falling rocks and catch the blinking carrots."

    def __init__(self, width=2 * PANEL_WIDTH, rng=None):
        super().__init__(width, rng)
        self.x = (width - 5) / 2
        self.rocks = []
        self.carrots = []
        self.clock = 0.0
        self.rock_wait = 0.0
        self.carrot_wait = 1.0
        self.move_wait = 0.0

    def press(self, key):
        if key in (LEFT, RIGHT):
            self.move(-1 if key == LEFT else 1)
            self.move_wait = 0.12

    def move(self, dx):
        self.x = float(min(max(0, self.x + dx), self.width - 5))

    def speed(self):
        """Rows a second the rocks fall; faster the longer you last."""
        return 9.0 + min(16.0, self.clock * 0.25)

    def rabbit_cells(self):
        x = int(round(self.x))
        return {(x + dx, HEIGHT - 6 + dy) for dy, row in enumerate(RABBIT) for dx, c in enumerate(row) if c == "1"}

    def update(self, dt, held=()):
        if self.over:
            return
        self.clock += dt
        self.move_wait -= dt
        while self.move_wait <= 0 and (LEFT in held) != (RIGHT in held):
            self.move(-1 if LEFT in held else 1)
            self.move_wait += 0.06
        if self.move_wait < 0:
            self.move_wait = 0.0
        self.rock_wait -= dt
        if self.rock_wait <= 0:
            self.rocks.append([self.rng.randrange(0, self.width - 1), -2.0])
            # More rocks the wider the field and the faster the game
            self.rock_wait += 7.0 / self.speed() * (18.0 / self.width) ** 0.5
        self.carrot_wait -= dt
        if self.carrot_wait <= 0:
            self.carrots.append([self.rng.randrange(0, self.width), -1.0])
            self.carrot_wait += 2.0 + self.rng.random()
        fall = self.speed() * dt
        for thing in self.rocks + self.carrots:
            thing[1] += fall
        rabbit = self.rabbit_cells()
        for rock in self.rocks:
            y = int(rock[1])
            if {(rock[0] + dx, y + dy) for dx in (0, 1) for dy in (0, 1)} & rabbit:
                self.over = True
        for carrot in self.carrots[:]:
            if (carrot[0], int(carrot[1])) in rabbit or (carrot[0], int(carrot[1]) + 1) in rabbit:
                self.carrots.remove(carrot)
                self.score += 1
        self.rocks = [r for r in self.rocks if r[1] < HEIGHT]
        self.carrots = [c for c in self.carrots if c[1] < HEIGHT]

    def draw(self):
        grid = np.zeros((HEIGHT, self.width))
        # Marks at the edges that scroll by, to show the speed
        offset = int(self.clock * self.speed())
        for y in range(HEIGHT):
            if (y - offset) % 6 == 0:
                grid[y, 0] = grid[y, self.width - 1] = 0.12
        for rock in self.rocks:
            y = int(rock[1])
            for dx in (0, 1):
                for dy in (0, 1):
                    if 0 <= y + dy < HEIGHT:
                        grid[y + dy, rock[0] + dx] = 0.55
        for carrot in self.carrots:
            y = int(carrot[1])
            if 0 <= y < HEIGHT:
                grid[y, carrot[0]] = 1.0 if (self.clock * 6) % 2 < 1 else 0.7
            if 0 <= y - 1 < HEIGHT:
                grid[y - 1, carrot[0]] = 0.3
        for x, y in self.rabbit_cells():
            grid[y, x] = 1.0
        return grid


GAMES = [Snake, Ledris, Breakout, RabbitRun]
GAMES_BY_KEY = {g.key: g for g in GAMES}


class GameSession:
    """One game being played: a 3-2-1 count-in, pause, and the score screen when it's over."""

    COUNT_IN = 1.5
    END_FLASH = 1.2

    def __init__(self, game, best=0, to_panels=None, panel_width=None):
        """to_panels turns the game's picture into what the panels show (white_rabbit/span.py),
        panel_width columns wide. The count-in, pause bars and score screen are drawn on that, so
        they sit on each panel the right way round."""
        self.game = game
        self.to_panels = to_panels or (lambda grid: grid)
        self.panel_width = panel_width or game.width
        self.best = best
        self.count_in = self.COUNT_IN
        self.paused = False
        self.ended = 0.0
        self.new_best = False

    @property
    def width(self):
        """How wide the picture on the panels is (9 per panel)."""
        return self.panel_width

    @property
    def finished(self):
        """The game is over and the score screen is showing."""
        return self.game.over and self.ended >= self.END_FLASH

    def press(self, key):
        if self.game.over or self.count_in > 0:
            return
        if not self.paused:
            self.game.press(key)

    def toggle_pause(self):
        self.set_paused(not self.paused)

    def set_paused(self, paused):
        self.paused = bool(paused) and not self.game.over

    def update(self, dt, held=()):
        if self.paused:
            return
        if self.count_in > 0:
            self.count_in -= dt
            return
        if self.game.over:
            if self.ended == 0.0 and self.game.score > self.best:
                self.best = self.game.score
                self.new_best = True
            self.ended += dt
            return
        self.game.update(dt, held)

    def frame(self):
        """The picture to show, brightness 0.0 - 1.0."""
        w = self.width
        if self.count_in > 0:
            grid = np.zeros((HEIGHT, w))
            number = str(int(np.ceil(self.count_in / self.COUNT_IN * 3)))
            for p in range(max(1, w // PANEL_WIDTH)):
                draw_digits(grid, number, p * PANEL_WIDTH + 3, 14)
            return grid
        grid = self.to_panels(self.game.draw())
        if self.paused:
            grid = grid * 0.25
            for p in range(max(1, w // PANEL_WIDTH)):
                x = p * PANEL_WIDTH
                grid[13:21, x + 2:x + 4] = 1.0
                grid[13:21, x + 5:x + 7] = 1.0
            return grid
        if self.game.over:
            if self.ended < self.END_FLASH:
                # The last picture blinks
                return grid if int(self.ended * 6) % 2 == 0 else grid * 0.2
            return self.score_screen()
        return grid

    def score_screen(self):
        """The score on the left panel (stacked digits); the best score on the right if there is one."""
        w = self.width
        grid = np.zeros((HEIGHT, w))
        score = str(min(self.game.score, 99999))
        draw_digits(grid, score, 3, max(0, (HEIGHT - len(score) * 6) // 2), 1.0, vertical=True)
        if w >= 2 * PANEL_WIDTH:
            best = str(min(self.best, 99999))
            level = 1.0 if (not self.new_best or int(self.ended * 3) % 2 == 0) else 0.3
            draw_digits(grid, best, PANEL_WIDTH + 3, max(0, (HEIGHT - len(best) * 6) // 2), level * 0.6, vertical=True)
            # A crown over the best score
            grid[1, PANEL_WIDTH + 2:PANEL_WIDTH + 7] = 0.6 * level
            grid[0, [PANEL_WIDTH + 2, PANEL_WIDTH + 4, PANEL_WIDTH + 6]] = 0.6 * level
        return grid


def load_high_scores(path=None):
    from led_mon import app_settings
    scores = app_settings.load_settings(path).get("high_scores")
    if not isinstance(scores, dict):
        return {}
    out = {}
    for key, value in scores.items():
        try:
            out[str(key)] = max(0, int(value))
        except (TypeError, ValueError):
            pass
    return out


def save_high_score(key, score, path=None):
    """Keep score as the best for this game if it beats the old one. Returns True if it did."""
    from led_mon import app_settings
    settings = app_settings.load_settings(path)
    scores = settings.get("high_scores") if isinstance(settings.get("high_scores"), dict) else {}
    if score <= int(scores.get(key, 0) or 0):
        return False
    scores[key] = int(score)
    settings["high_scores"] = scores
    return app_settings.save_settings(settings, path)
