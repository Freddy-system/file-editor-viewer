# File Editor & Viewer para herdr

[English](README.md) · Español

Para ver y editar archivos sin salir de la terminal: Markdown dibujado,
código resaltado y un editor integrado a una tecla. Funciona como **plugin de
[herdr](https://herdr.dev)** y como comando independiente, `fev`. Escrito en
Rust.

Dibuja Markdown completo (GFM): tablas, listas de tareas, alertas, notas al pie,
listas de definición, front matter, sub y superíndices, wikilinks, código con
resaltado, **diagramas Mermaid y Graphviz**, **fórmulas LaTeX** e imágenes.
Recarga solo cuando el archivo cambia.

## Instalar el plugin en herdr

Requisitos: herdr 0.9 o superior. La instalación descarga un binario ya
compilado para Linux y macOS; si no hay uno para tu plataforma, lo compila con
`cargo`.

Desde GitHub:

```bash
herdr plugin install Freddy-system/file-editor-viewer
```

Desde una copia local:

```bash
cargo build --release --bin fev
herdr plugin link /ruta/a/este/repositorio
```

Para tener el comando `fev` en la terminal:

```bash
cargo install --git https://github.com/Freddy-system/file-editor-viewer fev
```

Para abrirlo con una tecla, añade esto a `~/.config/herdr/config.toml` y ejecuta
`herdr server reload-config`:

```toml
[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "file-editor-viewer.open"
description = "vista previa de Markdown"
```

### Cómo se usa dentro de herdr

| Qué haces | Qué pasa |
|---|---|
| Pulsas la tecla configurada | Se abre un panel a la derecha con el `README.md` de la carpeta del panel activo, o el árbol de archivos si no hay README |
| Escribes `fev open notas.md` en un panel | Se abre ese archivo en un panel a la derecha |
| Escribes `fev open` en un panel | Se abre el README de la carpeta actual, o el árbol de archivos |
| Seleccionas una ruta `.md` y pulsas la tecla | Se abre ese archivo |
| Ctrl-clic sobre un enlace a un `.md` local | Se abre ese archivo |
| `herdr plugin action invoke file-editor-viewer.open-tab` | Lo mismo, en una pestaña nueva |
| `herdr plugin action invoke file-editor-viewer.open-overlay` | Lo mismo, en un panel superpuesto |

### Teclas del visor

| Tecla | Acción |
|---|---|
| `j` `k` `↑` `↓`, rueda | Desplazar |
| `d` `u`, `Espacio`, `RePág` `AvPág` | Media página / página |
| `Alt-↑` `Alt-↓` | Página arriba, página abajo |
| `Ctrl-↑` `Ctrl-↓` | Sección anterior, siguiente |
| `g` `G` | Inicio / final |
| `0` … `9` | Saltar a una décima parte: `5` la mitad, `9` casi al final |
| `/`, `n` `N` | Buscar, coincidencia siguiente / anterior |
| `/@nombre` | Buscar un archivo por nombre y abrirlo |
| `Tab`, `Mayús-Tab`, `Enter`, clic | Elegir y seguir enlaces (entre documentos y a títulos) |
| `Retroceso` | Volver al documento anterior |
| `t` | Índice del documento |
| `b` | Árbol de archivos lateral, con todos los archivos; `/` dentro busca un archivo |
| `e` | Editar el archivo con el editor integrado |
| Arrastrar con el ratón | Seleccionar texto; al soltar queda copiado |
| `y`, clic sobre un bloque de código | Copiar el bloque al portapapeles |
| `Y`, `Ctrl+A` | Copiar el documento entero (fuente Markdown) |
| `o` | Abrir en el navegador |
| `i` | Cambiar modo de imágenes (`kitty` / `text`) |
| `c` | Colores claros / oscuros |
| `?` | Ventana de ayuda con todas las teclas |
| `r`, `q` | Recargar, salir |

Los atajos con `Alt` funcionan desde cualquier sitio, también dentro del árbol
o de una búsqueda: `Alt-b` árbol, `Alt-f` buscar archivo, `Alt-t` índice,
`Alt-e` editar, `Alt-o` abrir en el navegador.

## Edición

El visor es de solo lectura hasta que pulsas `e`. Entonces abre un editor
integrado en la sección que estabas leyendo, que ocupa todo el panel; al salir
vuelves al documento ya dibujado. Funciona como un cuadro de texto
normal, sin modos y sin instalar ningún editor externo.

| Tecla | Acción |
|---|---|
| `Ctrl-S` | Guardar |
| `Esc`, `Ctrl-Q` | Volver a leer; avisa si hay cambios sin guardar |
| `Ctrl-Z`, `Ctrl-Y` | Deshacer, rehacer |
| `Mayús`+flechas, `Ctrl-A` | Seleccionar, seleccionar todo |
| `Ctrl-C`, `Ctrl-X`, `Ctrl-V` | Copiar, cortar, pegar |
| `Ctrl-K` | Borrar la línea |
| Clic, rueda | Mover el cursor, desplazar |
| `F1` | Todas las teclas del editor |

Cualquier archivo de texto (código, configuración) se abre en el propio visor
como listado resaltado y con números de línea, y se edita igual con `e`.

## Diagramas e imágenes: tres niveles

herdr solo ofrece a los plugins paneles de terminal, así que la calidad de los
gráficos depende del terminal en el que corre herdr:

1. **Modo `kitty`**: imagen real. Los diagramas y las fórmulas se dibujan como
   imágenes nítidas con el protocolo gráfico de Kitty. Funciona en Kitty,
   Ghostty, WezTerm y Konsole.
2. **Modo `text`**: para terminales sin imágenes, como Alacritty. Mermaid y
   Graphviz se dibujan con caracteres de caja, las fórmulas con símbolos Unicode y las
   imágenes con medios bloques de color.
3. **Navegador** (`o`): fidelidad completa en cualquier terminal. Usa Mermaid,
   KaTeX y Graphviz originales, con recarga en vivo.

fev elige el modo según el terminal que detecta; `i` lo cambia y recuerda la
elección. En `text`, los grafos de Graphviz conservan nodos, aristas
y etiquetas, pero no sus colores ni su disposición exacta.

Variables de entorno:

| Variable | Efecto |
|---|---|
| `FEV_GRAPHICS=kitty\|text` | Fuerza el modo de imágenes |
| `FEV_THEME=light\|dark` | Colores iniciales |
| `FEV_CELL=10x20` | Tamaño de celda en píxeles que se asume al dimensionar imágenes |
| `FEV_MOUSE=0` | No capturar el ratón (para seleccionar texto con el terminal) |

Si `plantuml` o `d2` están instalados, sus bloques de código también se dibujan.

## Uso fuera de herdr

```bash
fev README.md            # visor de terminal
fev docs/                # árbol de archivos de una carpeta
fev serve docs/          # vista previa en el navegador con recarga en vivo
fev export README.md     # escribe README.html
```

La vista de navegador carga Mermaid, KaTeX, highlight.js y Graphviz desde un
CDN, así que la primera vez necesita conexión. El servidor solo escucha en
`127.0.0.1` y no sirve nada fuera de la carpeta indicada.

## Arquitectura

Cada pieza es un crate independiente. Los frontends no se conocen entre sí;
todos parten del mismo modelo de documento.

```mermaid
flowchart TD
    core[fev-core<br/>parser y modelo del documento]
    render[fev-render<br/>Mermaid, Graphviz, LaTeX, SVG a píxeles]
    tui[fev-tui<br/>visor de terminal]
    web[fev-web<br/>página HTML y servidor]
    bin[fev<br/>binario y plugin de herdr]
    core --> tui
    core --> web
    render --> tui
    tui --> bin
    web --> bin
    web --> tui
```

| Crate | Responsabilidad |
|---|---|
| `fev-core` | Convierte Markdown en un árbol de bloques. Aquí se añade sintaxis nueva una sola vez para todos los frontends |
| `fev-render` | Motores de diagramas tras el trait `DiagramEngine`. Un lenguaje nuevo es un motor nuevo registrado en `Registry` |
| `fev-tui` | Maquetado a líneas de terminal, imágenes Kitty o texto, y la aplicación interactiva |
| `fev-web` | Página HTML autocontenida y servidor local con recarga en vivo |
| `fev` | Línea de comandos y el pegamento con herdr (`herdr-plugin.toml`) |

## Plan: VS Code y aplicación de escritorio

Aún no están implementados. Ambos reutilizan `fev-web`, que ya produce la
página completa, así que no añaden código de renderizado propio.

**Extensión de VS Code**

1. Extensión en TypeScript con un comando «fev: Abrir vista previa» que crea
   un `WebviewPanel` junto al editor.
2. La extensión arranca `fev serve` sobre la carpeta del workspace y carga la
   página en el webview; la recarga en vivo ya la da el servidor.
3. Sincronizar el desplazamiento editor ↔ vista previa mediante mensajes del
   webview, usando los `id` de los títulos que el núcleo ya genera.
4. Empaquetar el binario por plataforma dentro del `.vsix`. Alternativa sin
   binario: compilar `fev-core` y `fev-web` a WebAssembly.
5. Incluir Mermaid, KaTeX y highlight.js dentro de la extensión, porque los
   webviews bloquean los CDN por defecto.

**Aplicación de escritorio (Linux, Windows, macOS)**

1. Envoltorio con Tauri: una ventana cuyo contenido es la página de
   `fev-web`, con `fev-core` enlazado directamente en el proceso Rust.
2. Menú nativo, abrir archivo o carpeta, arrastrar y soltar, archivos recientes
   y asociación con la extensión `.md`.
3. Recursos web incluidos en el instalador para que funcione sin conexión.
4. Instaladores por plataforma (`.deb`, AppImage, `.msi`, `.dmg`) generados en CI.

Trabajo previo común a los dos: mover los recursos del CDN al propio paquete.

## Desarrollo

```bash
cargo test --workspace
cargo fmt --all
cargo run -- examples/demo.md
```

`examples/demo.md` recorre todo lo que el visor sabe dibujar.

El plugin declara `linux` y `macos`. Windows queda pendiente: el manifiesto
apunta a `target/release/fev`, sin `.exe`.

## Licencia

MIT © 2026 Freddy Sedano. Consulta [LICENSE](LICENSE). Cualquier copia o trabajo
derivado debe conservar el aviso de copyright y el texto de la licencia.
