# Calibración local de CPU, GPU y un ventilador compartido

Esta sesión mide cómo cambian las dos temperaturas al variar por separado la carga de CPU, la carga de GPU y el boost del ventilador. Dura 45 minutos, con una adquisición cada 2 segundos. El resultado es un intervalo de historial local que se puede ajustar con `identify`.

## Comprobación del equipo

Usa una cuenta con los permisos de escritura habituales de Thermal Berry en un equipo Linux con el controlador compatible. Confirma en la documentación del equipo que **un solo ventilador refrigera ambas unidades**. La detección cuenta los ventiladores expuestos por el controlador. Confirma también que la GPU de cómputo corresponde a la GPU medida. Cierra la GUI desde la bandeja y cualquier otro controlador de ventilación antes de iniciar la sesión.

Desde la raíz del repositorio, con las dependencias disponibles localmente, ejecuta la comprobación con el mismo usuario y directorio de datos que la GUI.

```sh
cargo build --locked --offline --release -p thermal-berry-headless
target/release/thermal-berry-headless calibrate --check
```

La salida JSON indica `ready`, `scope`, el motivo de cualquier rechazo y una muestra de los sensores. La comprobación abre la SQLite local y solo lee el hardware. Para la sesión conjunta se requiere `ready: true`, `scope: "cpu+gpu"`, RPM positivas, boost legible y las dos parejas de temperatura y utilización. Una GPU en reposo puede medir 0 %. Una lectura ausente aparece como `null`. Cuando faltan las señales GPU al inicio, la CLI declara el alcance `cpu-only`. Un segundo proceso que use el mismo directorio de datos sale con código 2 e identifica al propietario del bloqueo.

## Sesión de 45 minutos

Mantén fija la alimentación y la configuración energética y usa la misma carga de cómputo local en cada repetición. Elige un ejecutable en primer plano que trabaje con la GPU seleccionada. Anota localmente su versión, argumentos y backend para poder repetir la medición. Conserva la supervisión del equipo durante la calibración.

Ejecuta la CLI desde la raíz del repositorio.

```sh
target/release/thermal-berry-headless calibrate
```

Inicia las cargas locales cuando aparezca la primera consigna. Ajusta las cargas de CPU y GPU de forma independiente según los niveles que indique la terminal. Detén ambos trabajos cuando la CLI indique el fin de las cargas o si la sesión se interrumpe. Para una sesión exclusivamente de CPU, usa `calibrate --cpu-only`.

| Entrada | Duración de cada escalón | Niveles sucesivos, repetidos hasta el final |
| --- | ---: | --- |
| Carga CPU | 4 minutos | 15 %, 55 %, 85 % |
| Carga GPU | 3 minutos | 85 %, 15 %, 55 % |
| Boost del ventilador | 2 minutos | 40 %, 80 %, 55 %, 70 % |

La CLI escribe el boost y verifica su lectura en cada tick. Los periodos distintos generan tanto CPU baja con GPU alta como CPU alta con GPU baja. En una sesión nominal se registran 1 350 muestras. La comprobación de cobertura exige cuatro niveles de boost separados por al menos cinco puntos, tres bandas medidas por carga (menos de 30 %, de 30 % a menos de 70 %, y desde 70 %) y ambas combinaciones cruzadas. Cada nivel o combinación necesita al menos tres observaciones. Un hueco superior a 4 segundos invalida la sesión.

## Interrupción y recuperación

La calibración obtiene un inhibidor de suspensión antes de aplicar un boost. Un sensor inválido, la pérdida de una lectura obligatoria, RPM nulas, una discrepancia en el boost, un salto de reloj fuera de tolerancia o una temperatura de 85 °C o más detienen la sesión. El límite de adquisición es de 500 ms. Si ya tomó el ventilador, un fallo provoca un intento de refrigeración máxima seguido de la devolución al firmware. La interfaz verifica y registra por separado los errores de escritura.

Usa Ctrl+C para interrumpir. SIGTERM y SIGHUP activan la misma limpieza. Con SIGKILL o un fallo del sistema, el sistema operativo libera el bloqueo de instancia, pero el controlador integrado puede conservar el último boost. En ese caso, detén las cargas y usa el modo deshabilitado de la GUI para devolver el ventilador al firmware. Comprueba el resultado antes de reanudar la calibración.

## Ajuste offline

Al finalizar, la CLI imprime los valores exactos de `--from` y `--to` en milisegundos Unix. Sustituye los dos marcadores con esos valores.

```sh
target/release/thermal-berry-headless identify --from INICIO_MS --to FIN_MS
```

La GUI y la CLI usan `$XDG_DATA_HOME/thermal-berry/thermal-berry.db` (por defecto, `~/.local/share/thermal-berry/thermal-berry.db` en Linux). El directorio tiene permisos 0700 y el archivo 0600. La base conserva los datos originales y añade las cargas del mismo tick en una transacción. El historial está sujeto a la retención configurada, de siete días por defecto. Ejecuta el ajuste antes de que caduque la sesión.

El ajuste imprime el informe JSON con los coeficientes, los errores por horizonte, los modelos de referencia, los intervalos empíricos a 30, 60 y 300 s y `accepted`. Un ajuste aceptado desde SQLite guarda las ecuaciones en una sola transacción. Un ajuste rechazado conserva la versión anterior. Los objetivos configurables son 70 °C para CPU y 75 °C para GPU. El ajuste offline no acciona ventiladores.

Para analizar una captura local en otro equipo usa JSONL, con una observación por línea, en orden temporal. Los campos opcionales se pueden omitir o declarar como `null`. Este es un ejemplo de formato de una sola observación.

```json
{"timestamp_ms":2000,"cpu_temp_c":53.0,"gpu_temp_c":49.0,"cpu_percent":15.0,"gpu_percent":84.0,"fan_percent":40.0,"rpm":2100,"mode":"manual"}
```

```sh
target/release/thermal-berry-headless identify --input sesion.jsonl
```

La importación entrega el informe por la salida estándar y conserva los modelos instalados en el equipo. El límite de entrada es de 16 MiB, 20 000 observaciones y 4 KiB por línea. Para obtener ese formato de una sesión Linux, puedes exportar un rango de la base con el cliente SQLite local. En su terminal, sustituye los marcadores por enteros y guarda el archivo en un directorio privado.

```sql
.mode list
.headers off
.once sesion.jsonl
SELECT json_object('timestamp_ms',timestamp_ms,'cpu_temp_c',cpu_temp_c,'gpu_temp_c',gpu_temp_c,
                   'cpu_percent',cpu_percent,'gpu_percent',gpu_percent,'fan_percent',fan_percent,
                   'rpm',rpm,'mode',nullif(mode,'unknown'),'mem_percent',mem_percent,
                   'gpu_mem_percent',gpu_mem_percent,'gpu_power_w',gpu_power_w)
FROM load_readings WHERE timestamp_ms BETWEEN INICIO_MS AND FIN_MS ORDER BY timestamp_ms;
```
