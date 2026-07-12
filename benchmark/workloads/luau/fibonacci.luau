local function fibonacci(value: number): number
    if value < 2 then return value end
    return fibonacci(value - 1) + fibonacci(value - 2)
end

local total = 0
for _ = 1, 30 do total += fibonacci(28) end
print(total)
