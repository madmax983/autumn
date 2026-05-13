-- Atomically applies a token-bucket consume on a Redis hash.
--
-- Keys:
--   KEYS[1] - the per-IP hash key
--
-- Arguments (all passed as strings):
--   ARGV[1] - burst capacity (float)
--   ARGV[2] - refill rate in tokens/second (float)
--   ARGV[3] - current wall-clock time in milliseconds (integer)
--
-- Returns a three-element array: {allowed, remaining_floor, retry_after_secs}
--   allowed:          1 if the request is permitted, 0 if denied
--   remaining_floor:  floor of remaining tokens (valid when allowed=1)
--   retry_after_secs: ceil of seconds until one token refills (valid when allowed=0)
local key = KEYS[1]
local burst = tonumber(ARGV[1])
local refill_per_sec = tonumber(ARGV[2])
local now_ms = tonumber(ARGV[3])

local data = redis.call('HMGET', key, 'tokens', 'ts')
local tokens = tonumber(data[1])
local last_ts = tonumber(data[2])

if tokens == nil then
    tokens = burst
    last_ts = now_ms
end

local elapsed_secs = (now_ms - last_ts) / 1000.0
tokens = math.min(burst, tokens + elapsed_secs * refill_per_sec)

local allowed = 0
local remaining = 0
local retry_after_secs = 0

if tokens >= 1.0 then
    tokens = tokens - 1.0
    allowed = 1
    remaining = math.floor(tokens)
else
    allowed = 0
    local deficit = 1.0 - tokens
    retry_after_secs = math.ceil(deficit / refill_per_sec)
    if retry_after_secs < 1 then retry_after_secs = 1 end
end

local expiry_secs = math.ceil(burst / refill_per_sec) + 2
redis.call('HSET', key, 'tokens', tostring(tokens), 'ts', tostring(now_ms))
redis.call('EXPIRE', key, expiry_secs)

return {allowed, remaining, retry_after_secs}
