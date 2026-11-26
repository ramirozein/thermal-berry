# Validación de la instrumentación y el ajuste térmico offline

La extracción del núcleo compartido, la adquisición por tick y la identificación conjunta se verificaron en Linux. El ensayo sintético supera los criterios M1 en ambas unidades. **La aceptación física de M0 y M1 sigue pendiente.** El Alienware disponible expone dos ventiladores independientes. El diagnóstico leyó las temperaturas, las cargas de CPU y NVIDIA, la potencia GPU y los datos de ambos ventiladores. Rechazó la topología antes de tomar el control. El historial anterior de este equipo carece de lecturas de carga que permitan reconstruir una sesión conjunta.

## Exactitud del modelo

El ensayo reproducible genera 1 350 muestras con dos ecuaciones térmicas conocidas, un paso de 2 s y ruido gaussiano independiente de 0,2 °C. Los escalones de las tres entradas cambian con periodos distintos. Las ecuaciones que generan los datos se escribieron por separado de la construcción de las características del estimador. También se comprueban una planta desacoplada y el caso con datos únicamente de CPU. [Ensayo de identificación](../crates/thermal-berry-core/tests/identification.rs)

Las dos ecuaciones usan la misma biblioteca de 17 términos. La implementación calcula diferencias finitas sobre temperaturas crudas. Durante el ajuste compara ecuaciones de 1, 5 y 15 ticks construidas sumando los incrementos crudos en ventanas solapadas. La selección de ventana y umbral STLSQ usa dos particiones cronológicas crecientes y reproduce las entradas de calibración en sus respectivos bloques de validación. El último 20 % queda reservado para evaluar el pronóstico. Para las bandas a 30, 60 y 300 s, se toma el percentil 90 del error absoluto de una predicción entrenada con el primer 60 % y evaluada en el bloque del 60 al 80 %. El informe muestra el radio en °C y la cobertura observada en el último bloque. Los orígenes de pronóstico se solapan. Todos los modelos reciben solo la temperatura y las entradas disponibles en el origen de cada pronóstico. [Identificación y evaluación](../crates/thermal-berry-core/src/identify.rs)

| Modelo | CPU a 2 s | GPU a 2 s | CPU a 60 s | GPU a 60 s | CPU a 300 s | GPU a 300 s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| SINDy conjunto | 0,269 | 0,260 | 2,775 | 2,216 | 12,242 | 8,782 |
| Persistencia | 0,313 | 0,297 | 3,742 | 3,123 | 9,763 | 7,821 |
| ARX lineal conjunto | 0,269 | 0,261 | 2,713 | 2,209 | 12,305 | 8,897 |
| RC independiente por unidad | 0,288 | 0,274 | 2,915 | 3,113 | 12,253 | 8,149 |
| RLS con olvido sobre los términos seleccionados | 0,269 | 0,260 | 2,774 | 2,217 | 12,238 | 8,785 |

Los valores son RMSE en °C. Hay 269 orígenes de pronóstico a un paso y 240 a un minuto. A 30 s, SINDy obtuvo 1,145 °C en CPU y 1,014 °C en GPU. Los límites de aceptación son estrictos por unidad, menos de 1,5 °C a un paso y menos de 3 °C a 60 s. ARX obtuvo el menor error a un minuto. A cinco minutos la persistencia superó a SINDy en ambas unidades. Un segundo escenario con escalones rápidos ajenos al pronóstico alcanzó 3,959 °C en CPU a 60 s y el ajuste se rechazó.

SINDy recuperó exactamente los seis términos activos de cada ecuación, con error relativo máximo del 0,756 % en CPU y del 2,767 % en GPU. La aceptación comprueba el número de términos activos y los signos de las derivadas respecto de ambas cargas y el ventilador en el rango medido. También comprueba el sentido del cambio pronosticado a un minuto al aumentar una entrada. En un ensayo adicional con los escalones exactos del protocolo M0, se recuperaron los mismos seis términos. El error máximo en coeficientes fue del 4,302 % en CPU y del 9,210 % en GPU. El RMSE a 60 s fue de 2,785 °C y 2,820 °C respectivamente. La planta sintética usa términos presentes en la biblioteca. La cuantización, las perturbaciones ambientales y la dinámica del firmware requieren medición física.

## Rendimiento

En una compilación Linux x86-64 optimizada, la mediana de 15 ajustes completos fue de **9,59 ms** para 1 350 muestras. Incluye selección temporal y evaluación de las referencias. El pronóstico conjunto de 5 minutos promedió **3,38 µs** sobre 10 000 llamadas, y el modelo de coeficientes serializado ocupó **364 bytes**. Las cifras dependen de la máquina y de la sesión. El ensayo se ejecuta desde la raíz.

```sh
cargo test --locked --offline --release -p thermal-berry-core --test identification -- --include-ignored --nocapture --test-threads=1
```

## Pruebas del workspace

Pasaron las 93 pruebas ordinarias del workspace en Linux. Las dos pruebas opcionales, lectura real de Alienware y rendimiento en modo optimizado, se ejecutaron por separado. También pasaron la compilación del frontend, Clippy para el núcleo y la CLI, y la compilación de comprobación de ambas aplicaciones sin NVML.

```sh
bun install --frozen-lockfile --offline
bun run build
cargo test --locked --offline --workspace
cargo clippy --locked --offline -p thermal-berry-core -p thermal-berry-headless --all-targets -- -D warnings
```

La publicación y firma de instaladores quedan sin verificar. El flujo de publicación de la rama base ya referencia `tests/signed-release.sh`, ausente en ese árbol.

## Adquisición, almacenamiento y seguridad

- La GUI y la CLI adquieren el mismo bloqueo antes de abrir SQLite. En el arranque real de la GUI se observaron ticks con carga CPU en una base temporal, y una CLI competidora salió con código 2. Otra prueba verifica la liberación tras la muerte del propietario. Las pruebas de SQLite comprueban la escritura conjunta de cada tick, la retención y la reversión de ambas ecuaciones si falla la escritura de la segunda. [Prueba entre procesos](../crates/thermal-berry-headless/tests/exclusion.rs) · [Prueba del ajuste desde la CLI](../crates/thermal-berry-headless/tests/identify.rs)
- Las pruebas de control cubren emergencia a 90 °C, liberación a 85 °C en Adaptive, lecturas ausentes o inválidas, adquisición bloqueada, efectos de la suspensión y fallos de escritura. En calibración se prueban RPM cero, temperaturas de 85 °C, cargas inválidas, lectura de boost discrepante y devolución al firmware al terminar o ante un fallo. Las escrituras de estas pruebas se realizaron con ventiladores simulados. [Control térmico](../crates/thermal-berry-core/src/automatic.rs) · [Calibración](../crates/thermal-berry-core/src/calibration.rs)
- NVIDIA se comprobó con el controlador real en modo de lectura. La revisión automática rechazó la descarga de `nvml-wrapper` desde crates.io por las restricciones de servicios externos del proyecto. La implementación carga la biblioteca NVML local mediante `libloading`, ya presente en el bloqueo de dependencias, y consulta el subconjunto de lectura del ABI. Si faltan la biblioteca o las señales, el sistema puede declarar el alcance solo CPU. AMD se validó con un árbol sysfs simulado, incluida la ausencia de carga y la conversión de unidades. La validación en una GPU AMD física queda pendiente. [API de NVML](https://docs.nvidia.com/deploy/nvml-api/) · [Monitorización de AMDGPU](https://docs.kernel.org/gpu/amdgpu/thermal.html)
- La compilación de comprobación para Windows x86-64 incluyó el escritorio y la CLI. Las pruebas nativas de Windows y macOS quedan pendientes. La CLI puede ajustar un JSONL local en esas plataformas. [Investigación del control en Windows](OPERATING_SYSTEMS.md#control-de-ventilación-en-windows)

Para cerrar la aceptación se necesita una sesión real de 45 minutos en una máquina compatible con un ventilador compartido. La CLI verifica cobertura, excitación, calidad y monotonía antes de guardar el modelo. [Protocolo de la sesión](m0-calibration-session-protocol.md)
