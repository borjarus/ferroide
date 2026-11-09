## FerroIDE — Okna narzędziowe: Projekt, Problemy, Terminal, Uruchamianie, Debugowanie, Struktura, TODO, Powiadomienia, Usługi.
## Każdy komunikat musi istnieć także w en/panels.ftl (sprawdzane testem).

## Wspólne
panels-loading = Wczytywanie…
panels-load-failed = Nie udało się wczytać
panels-refresh = Odśwież
panels-time-now = przed chwilą
panels-time-minutes = { $n } min temu
panels-time-hours = { $n } godz. temu
panels-time-days = { $n ->
    [one] { $n } dzień temu
   *[other] { $n } dni temu
}

## Projekt
panels-project-open = Otwórz
panels-project-new-directory = Nowy katalog…
panels-project-new-directory-in = Nowy katalog w { $dir }
panels-project-rename = Zmień nazwę…
panels-project-delete = Usuń
panels-project-delete-title = Usunąć?
panels-project-delete-body = Usunąć „{ $name }”? Tej operacji nie można cofnąć.
panels-project-copy-path = Kopiuj ścieżkę
panels-project-copy-relative-path = Kopiuj ścieżkę względną
panels-project-name-placeholder = Nazwa (Enter zatwierdza, Esc anuluje)
panels-project-matches = { $count ->
    [one] { $count } dopasowanie
    [few] { $count } dopasowania
   *[other] { $count } dopasowań
}

## Problemy
panels-problems-errors = Błędy: { $count }
panels-problems-warnings = Ostrzeżenia: { $count }
panels-problems-infos = Informacje: { $count }
panels-problems-sort-severity = Sortuj według ważności
panels-problems-count = { $count ->
    [one] { $count } problem
    [few] { $count } problemy
   *[other] { $count } problemów
}
panels-problems-empty = Nie znaleziono problemów

## Terminal
panels-terminal-close = Zamknij sesję
panels-terminal-empty = Brak sesji terminala. Naciśnij +, aby otworzyć nową.
panels-terminal-readonly = Podgląd tylko do odczytu: interaktywny terminal pojawi się w etapie 8.

## Uruchamianie
panels-run-console = Konsola
panels-run-tests = Testy
panels-run-hide-passed = Ukryj zaliczone testy
panels-run-no-tests = Brak wyników testów
panels-run-select-test = Wybierz test, aby zobaczyć jego wyjście.
panels-run-summary = zaliczone: { $passed }, niezaliczone: { $failed }, pominięte: { $ignored }

## Debugowanie
panels-debug-resume = Wznów
panels-debug-pause = Wstrzymaj
panels-debug-step-over = Przejdź dalej
panels-debug-step-into = Wejdź do
panels-debug-step-out = Wyjdź z
panels-debug-stop = Zatrzymaj
panels-debug-frames = Ramki
panels-debug-console = Konsola
panels-debug-breakpoints = Punkty przerwania
panels-debug-paused = wstrzymany
panels-debug-running = działa
panels-debug-library-frames = { $count ->
    [one] Pokaż { $count } ramkę biblioteki
    [few] Pokaż { $count } ramki bibliotek
   *[other] Pokaż { $count } ramek bibliotek
}
panels-debug-no-variables = Brak zmiennych w tej ramce
panels-debug-no-breakpoints = Brak punktów przerwania
panels-debug-remove-breakpoint = Usuń punkt przerwania
panels-debug-evaluate = Oblicz wyrażenie (Enter)
panels-debug-mock-step = { $step }: niedostępne w usługach testowych (etap 10).

## Struktura
panels-structure-filter = Filtruj symbole
panels-structure-no-file = Otwórz plik, aby zobaczyć jego strukturę
panels-structure-empty = Brak symboli w tym pliku

## TODO
panels-todo-summary = TODO: { $todo }, FIXME: { $fixme }
panels-todo-count = { $count }
panels-todo-empty = Brak komentarzy TODO i FIXME

## Powiadomienia
panels-notifications-mark-read = Oznacz wszystkie jako przeczytane
panels-notifications-clear = Wyczyść wszystkie
panels-notifications-empty = Brak powiadomień

## Usługi
panels-services-running = działa
panels-services-stopped = zatrzymana
panels-services-failed = błąd
panels-services-empty = Brak usług
