## FerroIDE — polskie teksty interfejsu.
## Każdy komunikat musi istnieć także w en/ferro.ftl (sprawdzają to testy).

## Menu
menu-file = Plik
menu-view = Widok
menu-window = Okno
menu-run = Uruchom
menu-help = Pomoc
menu-tool-windows = Okna narzędziowe
menu-appearance = Wygląd
menu-language = Język
menu-keymap = Mapa klawiszy
menu-navigate = Nawigacja
menu-code = Kod
menu-vcs = Git

## Kategorie akcji (Znajdź akcję)
category-file = Plik
category-view = Widok
category-navigate = Nawigacja
category-code = Kod
category-vcs = Kontrola wersji
category-tool-windows = Okna narzędziowe
category-editor = Edytor
category-run = Uruchamianie
category-appearance = Wygląd
category-help = Pomoc

## Akcje
action-new-project = Nowy projekt…
action-open-project = Otwórz…
action-get-from-vcs = Pobierz z VCS…
action-close-project = Zamknij projekt
action-new-window = Nowe okno
action-open-settings = Ustawienia…
action-quit = Zakończ
action-find-action = Znajdź akcję…
action-search-everywhere = Szukaj wszędzie
action-hide-all-tool-windows = Ukryj wszystkie okna narzędziowe
action-maximize-tool-window = Maksymalizuj okno narzędziowe
action-reset-layout = Przywróć domyślny układ
action-toggle-compact-mode = Tryb kompaktowy
action-zoom-in-ui = Powiększ IDE
action-zoom-out-ui = Pomniejsz IDE
action-reset-ui-zoom = Resetuj powiększenie IDE
action-toggle-project = Projekt
action-toggle-commit = Commit
action-toggle-structure = Struktura
action-toggle-run = Uruchom
action-toggle-debug = Debugowanie
action-toggle-problems = Problemy
action-toggle-git = Git
action-toggle-terminal = Terminal
action-toggle-todo = TODO
action-toggle-notifications = Powiadomienia
action-toggle-database = Baza danych
action-toggle-build = Budowanie
action-close-tab = Zamknij kartę
action-close-other-tabs = Zamknij pozostałe karty
action-close-all-tabs = Zamknij wszystkie karty
action-pin-tab = Przypnij / odepnij kartę
action-next-tab = Następna karta
action-previous-tab = Poprzednia karta
action-split-right = Podziel w prawo
action-split-down = Podziel w dół
action-run = Uruchom
action-debug = Debuguj
action-stop = Zatrzymaj
action-theme-dark = Ferro Dark
action-theme-light = Ferro Light
action-theme-high-contrast = Wysoki kontrast
action-language-english = Język: angielski
action-language-polish = Język: polski
action-keymap-jetbrains = Mapa klawiszy: JetBrains (IntelliJ IDEA)
action-keymap-zed = Mapa klawiszy: Zed
action-keymap-sublime = Mapa klawiszy: Sublime Text
action-keymap-emacs = Mapa klawiszy: Emacs
action-toggle-vim-mode = Przełącz tryb Vim
action-new-file = Nowy plik…
action-open-settings-file = Otwórz plik ustawień (settings.toml)
action-open-keymap-settings = Skróty klawiszowe
action-toggle-left-dock = Pokaż/ukryj lewy panel
action-toggle-bottom-dock = Pokaż/ukryj dolny panel
action-toggle-right-dock = Pokaż/ukryj prawy panel
action-quick-open = Przejdź do pliku…
action-command-palette = Pokaż wszystkie polecenia
action-go-to-symbol-in-file = Przejdź do symbolu w pliku…
action-go-to-symbol-in-workspace = Przejdź do symbolu w projekcie…
action-go-back = Wstecz
action-go-forward = Dalej
action-go-to-line = Przejdź do wiersza…
action-find-in-files = Znajdź w plikach…
action-replace-in-files = Zamień w plikach…
action-select-opened-file = Zaznacz otwarty plik
action-next-problem = Następny problem
action-previous-problem = Poprzedni problem
action-trigger-completion = Pokaż podpowiedzi
action-quick-fix = Szybka poprawka…
action-quick-documentation = Szybka dokumentacja
action-parameter-info = Informacje o parametrach
action-rename-symbol = Zmień nazwę symbolu…
action-toggle-breakpoint = Przełącz punkt przerwania
action-new-terminal = Nowy terminal
action-toggle-services = Usługi
action-reopen-closed-tab = Otwórz ponownie zamkniętą kartę
action-edit-run-configurations = Edytuj konfiguracje uruchomieniowe…
action-show-diff = Pokaż różnice
action-resolve-conflicts = Rozwiąż konflikty…
action-keymap-vscode = Mapa klawiszy: VS Code
action-show-welcome = Ekran powitalny
action-about = O programie FerroIDE

## Okna narzędziowe
tool-project = Projekt
tool-commit = Commit
tool-structure = Struktura
tool-run = Uruchom
tool-debug = Debugowanie
tool-problems = Problemy
tool-git = Git
tool-terminal = Terminal
tool-todo = TODO
tool-notifications = Powiadomienia
tool-database = Baza danych
tool-build = Budowanie
tool-empty = Na razie nic tu nie ma
tree-loading = Wczytywanie…
tree-load-failed = Nie udało się wczytać
run-empty = Nic jeszcze nie uruchomiono. Naciśnij Uruchom na pasku narzędzi.
run-finished = Proces zakończył się z kodem { $code }
debug-empty = Brak aktywnej sesji debugowania
database-empty = Nie skonfigurowano źródeł danych
commit-changes = Zmiany
commit-message = Opis commita
commit-button = Commit

## Główny pasek narzędzi
toolbar-project = Projekt
toolbar-branch = Gałąź Git
toolbar-run-config = Konfiguracja uruchomieniowa
toolbar-settings = Ustawienia
toolbar-main-menu = Menu główne
dock-move-left = Zadokuj po lewej
dock-move-bottom = Zadokuj na dole
dock-move-right = Zadokuj po prawej
run-no-config = Dodaj konfigurację…
vcs-no-branch = Brak VCS
vcs-loading = Wczytywanie gałęzi…
vcs-local-branches = Lokalne
vcs-remote-branches = Zdalne

## Edytor
editor-loading = Otwieranie pliku…
editor-load-failed = Nie udało się otworzyć pliku
tab-pinned-cannot-close = Przypiętych kart się nie zamyka. Najpierw ją odepnij.

## Pasek stanu
status-cursor = Pozycja kursora (wiersz:kolumna)
status-line-separator = Separator linii
status-encoding = Kodowanie pliku
status-indent = Wcięcia
status-indent-4 = 4 spacje
status-language = Język pliku
status-memory = { $mb } MB
status-memory-tooltip = Pamięć używana przez FerroIDE (bez serwerów językowych)
lsp-ready = OK
lsp-starting = uruchamianie
lsp-crashed = awaria
task-indexing = Indeksowanie…

## Znajdź akcję
find-action-placeholder = Wpisz nazwę akcji…

## Ustawienia
settings-title = Ustawienia
settings-theme = Motyw
settings-language = Język
settings-keymap = Mapa klawiszy
settings-ui-size = Rozmiar czcionki IDE
settings-layout = Układ
settings-stage-note = Pełne okno ustawień pojawi się w Etapie 2.

## O programie
about-version = Wersja { $version }
about-stage = Podgląd Etapu 2 (M1 „UI Preview”): wszystkie główne widoki IDE, działające na usługach testowych (mockach).
about-theme-switch = Ostatnia zmiana motywu: { $took }

## Ekran powitalny
welcome-title = Witaj w FerroIDE
welcome-projects = Projekty
welcome-recent = Ostatnie projekty
welcome-search = Szukaj projektów
welcome-no-projects = Brak ostatnich projektów
welcome-today = dzisiaj
welcome-yesterday = wczoraj
welcome-days-ago = { $days ->
    [one] { $days } dzień temu
   *[other] { $days } dni temu
}

## Stage 2 shared
feature-coming-soon = Ten widok nie jest jeszcze dostępny.
tool-services = Usługi
