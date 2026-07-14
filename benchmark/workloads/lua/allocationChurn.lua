local total = 0
for index = 1, 20000 do
    local values = {}
    for slot = 1, 256 do values[slot] = index end
    total = total + values[1]
end
print(total)
