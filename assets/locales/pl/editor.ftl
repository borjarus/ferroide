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

## Pliki: zapisywanie, kodowania, separatory wierszy, zmiany na dysku (etap 3).
editor-modified = Zmieniony
editor-saved = Zapisano { $file }
editor-save-failed = Nie udało się zapisać { $file }: { $error }
editor-save-unmappable = { $encoding } nie obsługuje znaku „{ $char }” (znaków: { $count }). Wybierz inne kodowanie na pasku stanu.
editor-read-only = Tylko do odczytu
editor-binary-file = Ten plik wygląda na binarny.
editor-open-anyway = Otwórz jako tekst
editor-large-file = Duży plik: podświetlanie składni i analiza kodu są wyłączone.
editor-huge-file = Bardzo duży plik: pokazano pierwsze { $mb } MB, tylko do odczytu.
editor-decode-errors = Niektóre bajty nie są poprawne w kodowaniu { $encoding } i są pokazane jako �. Zapis by je zastąpił.
editor-mixed-line-endings = Plik miesza separatory wierszy; zapis użyje { $ending }.
editor-changed-on-disk = Plik { $file } zmienił się na dysku.
editor-changed-on-disk-dirty = Plik { $file } zmienił się na dysku, a tutaj ma niezapisane zmiany.
editor-reload = Wczytaj ponownie
editor-compare = Porównaj
editor-keep-mine = Zachowaj moje
editor-deleted-on-disk = Plik { $file } został usunięty z dysku. Zapis utworzy go ponownie.
editor-reloaded = Wczytano ponownie { $file } z dysku.
editor-diff-disk = Dysk
editor-diff-editor = Edytor
status-line-separator-menu = Separator wierszy tego pliku
status-encoding-reopen = Otwórz ponownie z kodowaniem
status-encoding-save = Zapisz z kodowaniem
