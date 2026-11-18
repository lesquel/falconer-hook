//! TUI strings. The English text is the key; other languages map it to
//! their translation and fall back to English when one is missing. Command
//! output (doctor details, setup logs) stays as the CLI prints it.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Es,
}

impl Lang {
    /// `LC_ALL`, then `LC_MESSAGES`, then `LANG`: the first one set and not
    /// empty decides; Spanish when it starts with `es`.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Lang {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| get(k).filter(|v| !v.is_empty()))
            .unwrap_or_default();
        if locale.starts_with("es") {
            Lang::Es
        } else {
            Lang::En
        }
    }
}

/// The translation of the English text `en`.
pub fn t(lang: Lang, en: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Es => ES.iter().find(|(k, _)| *k == en).map_or(en, |(_, es)| es),
    }
}

/// Whether `en` has a Spanish translation.
#[cfg(test)]
pub fn has(en: &str) -> bool {
    ES.iter().any(|(k, _)| *k == en)
}

/// `t` with each `{}` replaced by the next of `args`.
pub fn tf(lang: Lang, en: &'static str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut parts = t(lang, en).split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for p in parts {
        out.push_str(args.next().copied().unwrap_or_default());
        out.push_str(p);
    }
    out
}

/// English → Spanish (neutral).
#[rustfmt::skip]
static ES: &[(&str, &str)] = &[
    // Screens.
    ("Home", "Inicio"),
    ("Agents", "Agentes"),
    ("Phones", "Teléfonos"),
    ("Integrations", "Integraciones"),
    ("Service", "Servicio"),
    ("Settings", "Ajustes"),
    ("Usage", "Uso"),
    ("Advanced", "Avanzado"),
    ("Agents & inbox", "Agentes y bandeja"),
    ("Setup wizard", "Asistente de configuración"),
    // Fixes.
    ("Install Herdr or tmux", "Instalar Herdr o tmux"),
    ("Install Tailscale", "Instalar Tailscale"),
    ("Start Tailscale (sudo tailscale up)", "Iniciar Tailscale (sudo tailscale up)"),
    ("Install the OpenSSH server", "Instalar el servidor OpenSSH"),
    ("Turn the SSH server on", "Activar el servidor SSH"),
    ("Make the config and state private", "Hacer privados la configuración y el estado"),
    ("SSH: keys only, no passwords", "SSH: solo claves, sin contraseñas"),
    ("Install the user service", "Instalar el servicio de usuario"),
    ("Pair a phone", "Emparejar un teléfono"),
    ("Homebrew or your package manager when present, else the official installer {}, downloaded over HTTPS to a private file and run with sh.",
     "Homebrew o tu gestor de paquetes si existen; si no, el instalador oficial {}, descargado por HTTPS a un archivo privado y ejecutado con sh."),
    ("Homebrew when present, else the official installer {}, downloaded over HTTPS to a private file and run with sh.",
     "Homebrew si existe; si no, el instalador oficial {}, descargado por HTTPS a un archivo privado y ejecutado con sh."),
    ("Runs `sudo tailscale up`; open the login URL it prints.",
     "Ejecuta `sudo tailscale up`; abre la URL de inicio de sesión que muestra."),
    ("Installs the OpenSSH server with your package manager (sudo), then turns it on.",
     "Instala el servidor OpenSSH con tu gestor de paquetes (sudo) y luego lo activa."),
    ("Runs `sudo systemctl enable --now ssh` (macOS: `sudo systemsetup -setremotelogin on`).",
     "Ejecuta `sudo systemctl enable --now ssh` (macOS: `sudo systemsetup -setremotelogin on`)."),
    ("chmod 700 the config and state directories, 600 their files.",
     "chmod 700 a los directorios de configuración y estado, 600 a sus archivos."),
    ("Runs `falconer-hook harden-ssh`; sudo asks for your password.",
     "Ejecuta `falconer-hook harden-ssh`; sudo pide tu contraseña."),
    ("Runs `falconer-hook service install`.", "Ejecuta `falconer-hook service install`."),
    ("It runs in this terminal; the TUI comes back when it ends.",
     "Se ejecuta en esta terminal; la TUI vuelve cuando termina."),
    // Confirmations.
    ("Allow \"{}\"?", "¿Permitir \"{}\"?"),
    ("Deny \"{}\"?", "¿Denegar \"{}\"?"),
    ("Send \"{}\" to \"{}\"?", "¿Enviar \"{}\" a \"{}\"?"),
    ("Revoke {}? Its SSH key and push registration are removed.",
     "¿Revocar {}? Se eliminan su clave SSH y su registro de notificaciones."),
    ("Install the {} integration?", "¿Instalar la integración de {}?"),
    ("Uninstall the {} integration?", "¿Desinstalar la integración de {}?"),
    ("Run setup (config, user service, doctor)? Integrations are left as they are and nothing is installed.",
     "¿Ejecutar setup (configuración, servicio de usuario, doctor)? Las integraciones quedan como están y no se instala nada."),
    ("Run setup --yes: install every detected integration and every missing prerequisite without asking again?",
     "¿Ejecutar setup --yes: instalar todas las integraciones detectadas y todos los requisitos que faltan sin volver a preguntar?"),
    ("Official installers: {} and {} (Homebrew or your package manager first); sudo may ask for your password.",
     "Instaladores oficiales: {} y {} (primero Homebrew o tu gestor de paquetes); sudo puede pedir tu contraseña."),
    ("Clean up OpenMosh leftovers? Runs setup, which moves old hooks and replaces the old service.",
     "¿Limpiar los restos de OpenMosh? Ejecuta setup, que mueve los hooks antiguos y reemplaza el servicio anterior."),
    ("Fix {} problems: {}?", "¿Arreglar {} problemas: {}?"),
    ("Each fix shows its exact command and asks first.",
     "Cada arreglo muestra su comando exacto y pregunta antes."),
    ("Update Falconer to {}?", "¿Actualizar Falconer a {}?"),
    ("Downloads {} and runs it; it verifies the signed release with minisign and keeps your config and service.",
     "Descarga {} y lo ejecuta; verifica la versión firmada con minisign y conserva tu configuración y tu servicio."),
    ("Install (or reinstall) the user service?", "¿Instalar (o reinstalar) el servicio de usuario?"),
    ("Restart the user service?", "¿Reiniciar el servicio de usuario?"),
    ("Uninstall the user service? Phones stop getting notifications.",
     "¿Desinstalar el servicio de usuario? Los teléfonos dejan de recibir notificaciones."),
    ("Set the push endpoint of {} to {}?", "¿Cambiar el endpoint de notificaciones de {} a {}?"),
    ("Register this Web Push subscription for {}?", "¿Registrar esta suscripción Web Push para {}?"),
    ("Remove the Web Push subscription of {}?", "¿Quitar la suscripción Web Push de {}?"),
    ("Install the integration of every detected agent: {}?",
     "¿Instalar la integración de cada agente detectado: {}?"),
    // Messages.
    ("Setup wizard finished. Reopen it from Home.",
     "Asistente terminado. Puedes volver a abrirlo desde Inicio."),
    ("Cancelled; nothing changed.", "Cancelado; no cambió nada."),
    ("Uninstall cancelled; nothing changed.", "Desinstalación cancelada; no cambió nada."),
    ("Type \"{}\" to confirm; nothing changed.", "Escribe \"{}\" para confirmar; no cambió nada."),
    ("Empty input; nothing changed.", "Entrada vacía; no cambió nada."),
    ("Go to {}", "Ir a {}"),
    ("No matching action.", "Ninguna acción coincide."),
    ("falconer-hook was removed; press q to quit.", "falconer-hook se eliminó; pulsa q para salir."),
    ("Select a problem to fix.", "Selecciona un problema para arreglarlo."),
    ("Nothing to fix.", "No hay nada que arreglar."),
    ("Still checking for updates.", "Todavía se están buscando actualizaciones."),
    ("No update available.", "No hay actualizaciones."),
    ("No event selected.", "Ningún evento seleccionado."),
    ("That event is already resolved.", "Ese evento ya está resuelto."),
    ("No phone selected.", "Ningún teléfono seleccionado."),
    ("No agent selected.", "Ningún agente seleccionado."),
    ("Nothing to remove.", "No hay nada que eliminar."),
    ("{} is OK.", "{} está bien."),
    ("No automatic fix for {}: {}", "Sin arreglo automático para {}: {}"),
    ("Sent allow.", "Permiso enviado."),
    ("Sent deny.", "Denegación enviada."),
    ("Sent the reply.", "Respuesta enviada."),
    ("Done.", "Hecho."),
    ("Installed {} integrations.", "Se instalaron {} integraciones."),
    ("falconer-hook was removed. Press q to quit.", "falconer-hook se eliminó. Pulsa q para salir."),
    ("Some steps failed; see the list, then try again.",
     "Algunos pasos fallaron; revisa la lista y vuelve a intentarlo."),
    ("Press Enter to return to the TUI.", "Pulsa Enter para volver a la TUI."),
    // Footer and help.
    ("run", "ejecutar"),
    ("skip", "omitir"),
    ("leave", "volver"),
    ("help", "ayuda"),
    ("search", "buscar"),
    ("refresh", "actualizar"),
    ("quit", "salir"),
    ("Everywhere", "En todas partes"),
    ("Switch screens", "Cambiar de pantalla"),
    ("Move the selection", "Mover la selección"),
    ("Refresh", "Actualizar"),
    ("Search every action", "Buscar en todas las acciones"),
    ("Show or hide this help", "Mostrar u ocultar esta ayuda"),
    ("Quit (Esc closes a dialog first)", "Salir (Esc cierra antes un diálogo)"),
    ("Run this step (asks first)", "Ejecutar este paso (pregunta antes)"),
    ("Skip this step", "Omitir este paso"),
    ("Leave the wizard for Home", "Salir del asistente a Inicio"),
    ("Enter on list", "Enter en la lista"),
    ("Help", "Ayuda"),
    ("Search actions", "Buscar acciones"),
    // Wizard.
    ("done", "hecho"),
    ("checking", "comprobando"),
    ("to do", "pendiente"),
    ("Step {}/{}", "Paso {}/{}"),
    ("Done. Enter continues.", "Hecho. Enter continúa."),
    ("Status unknown. Enter checks again.", "Estado desconocido. Enter vuelve a comprobar."),
    ("Enter installs them, after asking.", "Enter las instala, después de preguntar."),
    ("Enter: {}, after asking.", "Enter: {}, después de preguntar."),
    ("s skips this step; Esc leaves the wizard for Home.",
     "s omite este paso; Esc sale del asistente a Inicio."),
    ("Prerequisites: Herdr or tmux", "Requisitos: Herdr o tmux"),
    ("SSH server", "Servidor SSH"),
    ("Tailscale up", "Tailscale activo"),
    ("Agent integrations", "Integraciones de agentes"),
    ("User service", "Servicio de usuario"),
    ("An agent runtime: Herdr (recommended, unlocks the agent view in the app) or tmux.",
     "Un entorno para agentes: Herdr (recomendado, activa la vista de agentes en la app) o tmux."),
    ("The phone reaches this machine over SSH, inside your tailnet.",
     "El teléfono llega a esta máquina por SSH, dentro de tu tailnet."),
    ("Tailscale must be installed, running and logged in; pairing listens only there.",
     "Tailscale debe estar instalado, en marcha y con sesión iniciada; el emparejamiento solo escucha ahí."),
    ("Each detected agent gets hooks that call falconer-hook emit; your other hooks stay.",
     "Cada agente detectado recibe hooks que llaman a falconer-hook emit; tus otros hooks se mantienen."),
    ("The user service runs falconer-hook serve, which watches agents and sends notifications.",
     "El servicio de usuario ejecuta falconer-hook serve, que vigila los agentes y envía notificaciones."),
    ("Scan a QR code with the Falconer app to add this machine.",
     "Escanea un código QR con la app Falconer para añadir esta máquina."),
    // Home.
    ("all good", "todo bien"),
    ("needs attention", "requiere atención"),
    ("unknown", "desconocido"),
    ("Checking for updates…", "Buscando actualizaciones…"),
    ("Up to date", "Al día"),
    ("v{} available", "v{} disponible"),
    ("Update check failed", "No se pudo buscar actualizaciones"),
    ("Version", "Versión"),
    ("Health:", "Salud:"),
    ("Health", "Salud"),
    ("(no automatic fix)", "(sin arreglo automático)"),
    ("Install what's missing", "Instalar lo que falta"),
    ("Update Falconer (v{} available)", "Actualizar Falconer (v{} disponible)"),
    ("Run setup wizard", "Abrir el asistente de configuración"),
    ("{} OK: {}", "{} bien: {}"),
    ("Quick actions", "Acciones rápidas"),
    // Screens' content.
    ("pending", "pendiente"),
    ("resolved", "resuelto"),
    ("Inbox", "Bandeja"),
    ("ssh", "ssh"),
    ("no ssh key", "sin clave ssh"),
    ("  (no notifications)", "  (sin notificaciones)"),
    ("  (endpoint stale)", "  (endpoint caducado)"),
    ("Paired phones", "Teléfonos emparejados"),
    ("Pair", "Emparejar"),
    ("p pairs a phone by QR over a one-shot, Tailscale-only listener in this terminal.",
     "p empareja un teléfono por QR con un receptor de un solo uso, solo por Tailscale, en esta terminal."),
    ("QR colors: {} (i inverts, for light terminal themes)",
     "Colores del QR: {} (i los invierte, para temas de terminal claros)"),
    ("dark on light", "oscuro sobre claro"),
    ("light on dark", "claro sobre oscuro"),
    ("detected", "detectado"),
    ("not found", "no encontrado"),
    ("installed", "instalado"),
    ("error", "error"),
    ("(none)", "(ninguno)"),
    ("Service status unknown; r refreshes.", "Estado del servicio desconocido; r actualiza."),
    ("The user service runs `falconer-hook serve`: it watches agents and sends notifications.",
     "El servicio de usuario ejecuta `falconer-hook serve`: vigila los agentes y envía notificaciones."),
    ("(no push endpoint)", "(sin endpoint de notificaciones)"),
    ("Push endpoint per phone", "Endpoint de notificaciones por teléfono"),
    ("Config (read-only)", "Configuración (solo lectura)"),
    ("Last {} days, from the agents' own local logs (+/- change the range).",
     "Últimos {} días, según los registros locales de cada agente (+/- cambian el rango)."),
    ("Token usage", "Uso de tokens"),
    ("Actions", "Acciones"),
    ("Output", "Salida"),
    ("Uninstall plan (dry run), --all: {}", "Plan de desinstalación (simulación), --all: {}"),
    ("yes", "sí"),
    ("no", "no"),
    // Dialogs.
    ("y confirm   n / Esc cancel", "y confirmar   n / Esc cancelar"),
    ("Confirm", "Confirmar"),
    ("Confirm uninstall", "Confirmar desinstalación"),
    ("This removes everything listed in the plan.", "Esto elimina todo lo que aparece en el plan."),
    ("Type \"{}\" and press Enter:", "Escribe \"{}\" y pulsa Enter:"),
    ("Esc cancel", "Esc cancelar"),
    ("Reply text", "Texto de respuesta"),
    ("Reply to \"{}\":", "Responder a \"{}\":"),
    ("Web Push subscription", "Suscripción Web Push"),
    ("Paste the subscription JSON for {} (as for devices set-webpush):",
     "Pega el JSON de la suscripción para {} (como en devices set-webpush):"),
    ("Push endpoint", "Endpoint de notificaciones"),
    ("New push endpoint URL for {}:", "Nueva URL de notificaciones para {}:"),
    ("Enter continue   Esc cancel", "Enter continuar   Esc cancelar"),
    // Actions: hints and labels.
    ("do", "hacer"),
    ("Do the selected item", "Hacer lo seleccionado"),
    ("fix", "arreglar"),
    ("Fix the selected problem", "Arreglar el problema seleccionado"),
    ("fix all", "arreglar todo"),
    ("wizard", "asistente"),
    ("update", "actualizar"),
    ("Update Falconer", "Actualizar Falconer"),
    ("allow", "permitir"),
    ("Allow the selected request", "Permitir la solicitud seleccionada"),
    ("deny", "denegar"),
    ("Deny the selected request", "Denegar la solicitud seleccionada"),
    ("reply", "responder"),
    ("Reply with text", "Responder con texto"),
    ("pair", "emparejar"),
    ("invert QR", "invertir QR"),
    ("Invert QR colors", "Invertir los colores del QR"),
    ("revoke", "revocar"),
    ("Revoke the selected phone", "Revocar el teléfono seleccionado"),
    ("web push", "web push"),
    ("Set the Web Push subscription", "Configurar la suscripción Web Push"),
    ("clear push", "quitar push"),
    ("Clear the Web Push subscription", "Quitar la suscripción Web Push"),
    ("install", "instalar"),
    ("Install the selected integration", "Instalar la integración seleccionada"),
    ("uninstall", "desinstalar"),
    ("Uninstall the selected integration", "Desinstalar la integración seleccionada"),
    ("restart", "reiniciar"),
    ("Restart the service", "Reiniciar el servicio"),
    ("Uninstall the user service", "Desinstalar el servicio de usuario"),
    ("endpoint", "endpoint"),
    ("Set the push endpoint of the selected phone", "Cambiar el endpoint de notificaciones del teléfono seleccionado"),
    ("more days", "más días"),
    ("Show more days", "Mostrar más días"),
    ("fewer days", "menos días"),
    ("Show fewer days", "Mostrar menos días"),
    ("Run the selected action", "Ejecutar la acción seleccionada"),
    ("SSH keys only", "SSH solo claves"),
    ("SSH: keys only, no passwords (sudo)", "SSH: solo claves, sin contraseñas (sudo)"),
    ("toggle --all", "alternar --all"),
    ("Toggle --all (also delete the binary)", "Alternar --all (también borra el binario)"),
    ("Uninstall Falconer", "Desinstalar Falconer"),
    ("Run setup (integrations as they are)", "Ejecutar setup (integraciones como están)"),
    ("Run setup and install every detected integration", "Ejecutar setup e instalar todas las integraciones detectadas"),
    ("Clean up OpenMosh leftovers", "Limpiar los restos de OpenMosh"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_keys_are_unique_and_keep_their_placeholders() {
        for (i, (en, es)) in ES.iter().enumerate() {
            assert!(
                !ES[..i].iter().any(|(k, _)| k == en),
                "duplicate key {en:?}"
            );
            assert!(!es.is_empty(), "{en:?}");
            assert_eq!(en.matches("{}").count(), es.matches("{}").count(), "{en:?}");
        }
    }

    #[test]
    fn tf_fills_placeholders_in_order() {
        assert_eq!(tf(Lang::En, "Step {}/{}", &["2", "6"]), "Step 2/6");
        assert_eq!(tf(Lang::Es, "Step {}/{}", &["2", "6"]), "Paso 2/6");
        assert_eq!(tf(Lang::En, "Go to {}", &[]), "Go to ");
    }
}
