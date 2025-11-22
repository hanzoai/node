#!/bin/bash

# Fix line 755 (col.clone() is the key)
sed -i '' '755,761s/Cell {/Cell {\n                        id: CellId::from(col.clone()),/' src/sheet.rs

# Fix line 800 (col.clone() is the key)
sed -i '' '800,806s/Cell {/Cell {\n                                id: CellId::from(col.clone()),/' src/sheet.rs

# Fix line 812 (col.clone() is the key)  
sed -i '' '812,818s/Cell {/Cell {\n                            id: CellId::from(col.clone()),/' src/sheet.rs

# Fix line 1022 (col.clone() is the key)
sed -i '' '1022,1028s/Cell {/Cell {\n                                id: CellId::from(col.clone()),/' src/sheet.rs

# Fix line 1049 (col.clone() is the key)
sed -i '' '1049,1055s/Cell {/Cell {\n                                id: CellId::from(col.clone()),/' src/sheet.rs

# Fix line 1114 (col.clone() is the key)
sed -i '' '1114,1120s/Cell {/Cell {\n                                    id: CellId::from(col.clone()),/' src/sheet.rs
