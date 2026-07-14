local total = 0
for index = 1, 20000 do
    local values = table.create(256, index)
    total += values[1]
end
print(total)
