## FerroIDE — Warstwa widoku edytora: margines, ścieżka, przyklejone wiersze, podpowiedzi, okienka.
## Każdy komunikat musi istnieć także w en/editor.ftl (sprawdzane testem).

editor-breadcrumb-tooltip = Przejdź do tego symbolu
editor-problem-count = ✕ { $errors }  ⚠ { $warnings }
editor-toolbar-search = Znajdź w pliku
editor-toolbar-soft-wrap = Zawijanie
editor-toolbar-inlay-hints = Podpowiedzi
editor-run-main = Uruchom lub debuguj main
editor-run-test = Uruchom lub debuguj { $name }
editor-status-indent = { $size ->
    [one] { $size } spacja
    [few] { $size } spacje
   *[other] { $size } spacji
}
editor-status-problems = Błędy i ostrzeżenia w tym pliku
