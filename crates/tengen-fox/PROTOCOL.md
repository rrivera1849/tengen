# Fox protocol notes

What Tengen relies on to talk to the Fox Go Server (野狐围棋). These are
protocol facts (message numbers, field numbers, the order of steps), learned by
reading [foxwq-web](https://github.com/foxwq-web/foxwq-web.github.io), a
working browser client whose authors in turn recovered the names from the
official Android client. No code was copied from it; Tengen's implementation
is its own.

Fox is unencrypted. Every message is a small binary frame carrying a
protobuf message, over plain TCP.

## Frames

```
u16 length                      (big-endian; 0x7fff means a long frame, below)
u16 message type
u8  flag                        0 = raw, 2 = zlib-compressed payload
..  payload                     protobuf
u32 Adler-32 of everything above except the length prefix
```

`length` counts message type + flag + payload + checksum (payload + 7).
A long frame starts `7fff 000b` followed by a `u32` length.

A zlib payload is a normal zlib stream (it ends with its own Adler-32 of the
inflated bytes).

## Signing in: the navigation server

1. Connect to `43.132.13.39:10000`.
2. Send type **2000**, a LoginRequest:
   `1` account name, `2` `"foxwq"`, `3` MD5 of the password (lowercase hex),
   `4` = 8, `5` = 11509231 (client version), `6` false, `7` a random
   12-hex-digit MAC address (uppercase), `9` false.
3. Receive type **2005**: `1` error code (0 = ok; 111231 = wrong name or
   password), `2` uid (or `3.1`), `3` account (`3.3` nickname, `3.2` name,
   `3.4` grade), `8` session token (bytes).
4. Send **3000** `{1: uid, 2: false}` and **3010** `{1: uid}`.
5. Receive **3005**: `3` repeated server records. The record with `1 = 1`
   (or the first) gives the gateway: `21` host, `22` port.

## The gateway

Everything else goes through the gateway as message type **26200**, whose
payload is:

```
u16 header length, header (FGWHead protobuf), body (protobuf)
```

FGWHead: `1` message id, `3` uid, `5` client type (32 = PC), `6` account
type (6), `7` server type, `8` server id, `19` client version
(`"11509231"`), `20` language (4).

Server types: **20001** gateway, **30006** room manager, **31000** game
server.

| Step | Server | Message | Body |
| --- | --- | --- | --- |
| Gateway login | 20001 | 1 | `1` session token; reply `1` op code (0 = ok) |
| Heartbeat, every 5 s | 20001 | 5 | `1` client ticks (ms) |
| Room manager login | 30006 | 100008 | `1` true; reply `1` result |
| Room manager heartbeat | 30006 | 100010 | `1` client ticks |
| Game list (fields) | 30006 | 100150 | reply `1` repeated FieldCfg |
| Unfinished game? | 30006 | 100115 | reply `1` result, `2` server type, `3` server id, `4` room id, `7` create time |
| Automatch | 30006 | 100130 | `1` field id, `2` false; reply `1` result |
| Automatch timed out | 30006 | 100132 | |
| Cancel automatch | 30006 | 100139 | reply/notify 100139, 100141 |
| Opponent found | 30006 | 100140 | `1` result, `2` server type, `3` server id, `4` room id, `7` create time |

FieldCfg: `1` field id, `2` board size, `3` main time (s), `4` byo-yomi
periods, `5` byo-yomi seconds, `7` additional rule (0 = plain), `12` game type
tag (1 = normal).

A room key is `{1 server type, 2 server id, 3 room id, 4 create time}`. Game
server messages are sent with the room's server id in the FGWHead.

## In a game (server 31000)

| Message | Meaning | Fields |
| --- | --- | --- |
| 105 | Ready (send after 100140) | |
| 107 | Game starts | `1` rule, `9` repeated players, `6/7/11/12` room id, create time, server type, id |
| 200 | Heartbeat (send while in a room) | `1` ticks, `3` room key |
| 201 | Play a stone (send) | `1` x, `2` y, `3` color, `4` `{1 sync id}` |
| 203 | A stone was played | `1` x, `2` y, `3` color, `4` clock, `5` `{1 sync id}` |
| 204 | Pass (send) | `1` `{1 sync id}` |
| 206 | A pass | `1` clock, `2` `{1 sync id}` |
| 108 | Clock update | clock fields at top level |
| 109 | Clock resumes | `2` clock |
| 301 | Ask to count the game (send) | `1` 0 |
| 303 | Counting needs an answer | `1` result, `2` stage (1 request, 2 result), `5` ownership (361 chars, 0/1/2), `8` winner |
| 304 | Answer counting (send) | `1` accept, `2` stage |
| 306 | Counting answered | `1` accept, `2` stage |
| 311 | Resign (send) | `1` your color, `2` room key |
| 402, 403, 405 | Game over | `2` winner (1 black, 2 white, 3 draw), `3` margin × 50 (−2 = on time), `4` reason (3 resignation, 8 void) |
| 501 | Fetch the whole game (send; also its reply) | reply `2` rule, `3` players, `4` moves, `5` `{1 sync id}`, `6` state (5 = over), `15` result |

Colors: 1 black, 2 white. Points are `(x, y)` from the top-left, 0-based.

Rule: `1` board size, `3` handicap, `4` komi × 50, `6` main time, `7`
byo-yomi seconds, `8` byo-yomi periods.

Player: `1` uid, `3` nickname, `4` grade, `6` color, `7` main time left,
`8` periods left, `9` seconds left in the period.

Clock: `2` main time left, `3` periods left, `4` seconds left in the period,
`5` uid.

Move (in 501): `1` flag (1 move, 2 pass, 3 handicap), `2` x, `3` y, `4` color.

Grade to rank: below 18 is `(18 − grade)k`, 18–26 is `(grade − 17)d`, 27–35 is
`(grade − 26)p`.

Every move and pass carries the last sync id the client saw. A stale id is
refused with error 109036; the fix is to fetch the whole game again (501).

Fox ends games by counting rather than by marking dead stones: one player asks
to count (301), the other accepts (304 stage 1), Fox's AI judges the board and
both confirm the result (303 then 304 stage 2), then the game-over message
arrives.

The gateway may close a session with message 100 (server 20001): `1` code
(51001 = signed in elsewhere), `8` reason.
