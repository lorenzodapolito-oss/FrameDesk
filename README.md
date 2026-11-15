# FrameDesk di Lorenzo D'Apolito

Analizzatore di qualità video, scritto in Rust con egui. Porta in Rust il «Video Quality Analyzer PRO» con le stesse formule, e in più:

- analisi **in parallelo** con pulsante **Annulla** e tempo massimo per video;
- **trascinamento** di file e cartelle nella finestra;
- **cache** dei risultati (senza miniature, quindi veloce da leggere);
- **ancore**: con almeno due video di riferimento i voti si confrontano con i *tuoi* migliori e peggiori;
- **report HTML** pastello con miniature incorporate;
- tutti i valori delle impostazioni vengono controllati (mai un numero fuori scala), nessun `unwrap`/`panic`.

## Come funziona

Per ogni video `ffprobe` legge le informazioni e `ffmpeg` estrae alcuni fotogrammi (più quelli ai cambi di scena). Su ognuno si misurano:

- **Purezza**: blocchettatura e ringing della compressione (100 − blocchi·2,2 − ringing·0,5);
- **Densità**: dettaglio reale con FFT, ridotto se sembra un upscaling;
- **Stabilità**: sfarfallio, fermi immagine ed efficienza del bitrate.

Il punteggio è la media pesata (0,40 / 0,35 / 0,25) e il voto va da *Scarso* a *Eccellente*.

## Requisiti

`ffmpeg` e `ffprobe`, accanto a `FrameDesk.exe` o nel PATH (altrimenti il pulsante «Scegli ffmpeg…»).

## Costruire

`costruisci-framedesk.cmd` (stessa cartella di AmbienteRust) scarica, compila e crea `dist\FrameDesk-portable`.

Colori: solo pastello, mai bianco né grigio (c'è un test che lo verifica anche per il report).
