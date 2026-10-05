# HNK-012

Порционная история HEAD с закреплённым immutable tip. Stateful gix traversal живёт
только на bounded worker: одна порция до 200 commits, без повторного прохода всех
старых commits на каждую следующую порцию. UI хранит ограниченное окно порций и
виртуальные строки. Новый HEAD/refresh начинает новую сессию, locale не читает Git.

Выбранный commit показывает сообщение/parents и только его изменённые файлы.
Root сравнивается с empty tree; остальные с явно выбранным parent, у merge
видны номер и полный OID базы. Rename detection отключена в этом slice: точный
delete/add, без скрытого чтения всех blobs ради похожести. Side-by-side использует
обычный owned diff, сравнивает raw Git blobs и modes, без текущих worktree attrs.
History никогда не меняет index/worktree; staging controls в этом просмотре выключены.

Core модели без GPUI/gix типов. В worker state может жить !Send gix service,
созданный внутри его потока. Generation/request offset связывают reply с запросом;
потерянная порция не должна незаметно перескочить вперёд. Показать предыдущую
порцию можно из bounded cache; большая история продолжает загружаться порционно.

Приёмка: SHA-1/SHA-256, root/linear/merge/octopus, explicit parents, packed/linked/
detached/unborn, dirty worktree/index invariance, NUL/newline/Cyrillic filenames,
binary/large/symlink/gitlink/modes, invalid object requests/encoding/size limits;
Git oracle log/show/diff-tree --no-renames. Native real Wayland pages/parent/files/
diff/locale selection и сохранение dirty Result при переходе в History.

API evidence: locked gix 0.88.0 sources, gix-traverse 0.62.0 Simple owned Find
iterator and gix tree changes; [revision walk](https://docs.rs/gix/0.88.0/gix/revision/walk/struct.Platform.html),
[Git log](https://git-scm.com/docs/git-log), [Git diff-tree](https://git-scm.com/docs/git-diff-tree).
Поддержка runtime и UI не выводится из успешной компиляции.
