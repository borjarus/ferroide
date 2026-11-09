## FerroIDE — Szybkie otwieranie, paleta poleceń i wyszukiwanie w plikach.
## Każdy komunikat musi istnieć także w en/search.ftl (sprawdzane testem).

## Szybkie otwieranie / paleta poleceń
search-query-label = Szybkie otwieranie
search-results-label = Wyniki
search-placeholder-files = Szukaj plików po nazwie
search-placeholder-commands = Wpisz nazwę polecenia
search-placeholder-symbols-file = Wpisz nazwę symbolu w tym pliku
search-placeholder-symbols-workspace = Wpisz nazwę symbolu w projekcie
search-placeholder-line = Wpisz numer wiersza, opcjonalnie wiersz:kolumna
search-placeholder-text = Wpisz tekst do wyszukania w projekcie
search-mode-files = Pliki
search-mode-commands = Polecenia
search-mode-symbols-file = Symbole w pliku
search-mode-symbols-workspace = Symbole w projekcie
search-mode-line = Przejdź do wiersza
search-mode-text = Wyszukiwanie tekstu
search-hint = > polecenia · @ symbole w pliku · # symbole w projekcie · : wiersz · % tekst
search-loading = Wyszukiwanie…
search-no-files = Brak pasujących plików
search-no-commands = Brak pasujących poleceń
search-no-symbols = Brak pasujących symboli
search-no-text = Brak wyników
search-no-editor = Otwórz plik w edytorze, aby użyć tego trybu
search-type-to-search = Zacznij pisać, aby wyszukać
search-line-hint = Wpisz numer wiersza od 1 do { $lines }; działa też wiersz:kolumna
search-line-invalid = „{ $input }” nie jest numerem wiersza
search-line-out-of-range = Wiersz { $line } jest poza plikiem (1–{ $lines })
search-line-target = Przejdź do wiersza { $line }, kolumna { $column }
search-recent = ostatnio otwierane
search-recent-command = ostatnio używane
search-preview-loading = Wczytywanie podglądu…
search-preview-failed = Podgląd niedostępny: { $error }
search-error = Wyszukiwanie nie powiodło się: { $error }
search-footer = ↑↓ wybór · Enter otwórz · { $split } otwórz obok · Esc zamknij

## Wyszukiwanie w plikach
search-fif-title-find = Znajdź w plikach
search-fif-title-replace = Zamień w plikach
search-fif-query = Szukany tekst
search-fif-replacement = Zamień na
search-fif-filter = Uwzględniane pliki (np. src/ lub .rs)
search-fif-case = Uwzględniaj wielkość liter (Alt+C)
search-fif-word = Tylko całe słowa (Alt+W)
search-fif-regex = Wyrażenie regularne (Alt+R)
search-fif-toggle-replace = Przełącz zamianę
search-fif-replace-all = Zamień wszystkie
search-fif-hint = Zacznij pisać, aby przeszukać projekt
search-fif-empty = Brak wyników
search-fif-summary = Wyniki: { $matches }, liczba plików: { $files }
search-fif-truncated = Pokazano tylko pierwsze wiersze ({ $limit })
search-fif-open-hint = Enter lub dwuklik otwiera wynik
search-fif-replaced = Zamieniono wystąpienia ({ $count }) w otwartych plikach ({ $files }).
search-fif-skipped = Pominięto pliki nieotwarte w edytorze ({ $skipped }): zapis plików pojawi się wraz z prawdziwą usługą projektu (etap 4).
search-fif-nothing-replaced = Nic nie zamieniono: najpierw otwórz plik zawierający wyniki.
