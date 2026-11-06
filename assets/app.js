
const icons={fox:"🦊",cat:"🐱",dog:"🐶",rabbit:"🐰"};
let token=null;
async function load(){
  const box=document.getElementById("pets"),error=document.getElementById("error");
  box.replaceChildren();error.textContent="";
  if(!token) return;
  try {
    const response=await fetch("/pets",{headers:{Authorization:"Bearer "+token}});
    if(!response.ok) throw new Error("Сервис вернул HTTP "+response.status);
    const pets=await response.json();
    for(const pet of pets){
      const card=document.createElement("article");card.className="card";
      const icon=document.createElement("div");icon.className="icon";icon.textContent=icons[pet.species]||"🐾";
      const title=document.createElement("h2");title.textContent=pet.name;
      const desc=document.createElement("p");desc.textContent=pet.biography;
      const id=document.createElement("small");id.textContent=pet.species+" · "+pet.id;
      card.append(icon,title,desc,id);box.append(card);
    }
    if(!pets.length) error.textContent="Для этого пользователя нет доступных карточек.";
  } catch(e){error.textContent="Не удалось загрузить карточки: "+e.message}
}
document.getElementById("sign-in").addEventListener("click",async()=>{
  const error=document.getElementById("error"),password=document.getElementById("password");
  error.textContent=""; token=null;
  try{
    const response=await fetch("/session",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({username:document.getElementById("who").value,password:password.value})});
    password.value="";
    if(!response.ok) throw new Error("Не удалось войти (HTTP "+response.status+")");
    token=(await response.json()).token; await load();
  }catch(e){password.value="";error.textContent=e.message}
});
document.getElementById("sign-out").addEventListener("click",async()=>{
  if(token) await fetch("/session",{method:"DELETE",headers:{Authorization:"Bearer "+token}});
  token=null;document.getElementById("pets").replaceChildren();
});
document.getElementById("who").addEventListener("change",()=>{token=null;document.getElementById("pets").replaceChildren()});
